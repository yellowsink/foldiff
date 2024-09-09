// performs diffing using zstd, similar to the --patch-from cli argument in the zstd cli

use anyhow::Result;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use zstd::dict::{DecoderDictionary, EncoderDictionary};
use zstd::{Decoder, Encoder};

fn length_of(stream: &mut impl Seek) -> Result<u64> {
	let current_pos = stream.stream_position()?;
	let length = stream.seek(SeekFrom::End(0))?;
	stream.seek(SeekFrom::Start(current_pos))?;
	Ok(length)
}

fn resolve_len(s: &mut impl Seek, l: Option<u64>) -> Result<u64> {
	l.map_or_else(|| length_of(s), Ok)
}

fn calc_chunk_num(
	s1: &mut impl Seek,
	s2: &mut impl Seek,
	l1: Option<u64>,
	l2: Option<u64>,
) -> Result<(f64, u64, u64, f64, f64)> {
	let l1 = resolve_len(s1, l1)?;
	let l2 = resolve_len(s2, l2)?;
	let l1f = l1 as f64;
	let l2f = l2 as f64;
	let num_chunks = l1f / ((1u64 << 31) as f64); // 2 GiB (2_147_483_648 bytes)
	let num_chunks = num_chunks.ceil(); // round up to ensure the chunk size is <=

	Ok((num_chunks, l1, l2, l1f, l2f))
}

fn calc_chunks(num_chunks_f: f64, len: f64) -> impl Iterator<Item = u64> {
	let chunk_size = len / num_chunks_f;
	let chunk_range = (0..num_chunks_f as u64).map(|i| i as f64);
	chunk_range.map(move |i| (i * chunk_size) as u64)
}

fn read_u64(r: &mut impl Read) -> Result<u64> {
	let mut buf = [0u8; 8];
	r.read_exact(&mut buf)?;
	Ok(u64::from_be_bytes(buf))
}

/// Creates a diff from `old` to `new`, and writes it into `dest`.
/// The diff structure (number of blobs, (length of blob, blob)[]) will be written into `dest` at the current seek point.
/// `level` is the zstd compression level, higher will give smaller diffs.
/// `old_len_hint` and `new_len_hint` should either not be provided, or MUST be EXACTLY the size of the old and new streams, and allows eliding length determination via SeekFrom::End.
pub fn diff(
	old: &mut (impl Read + Seek),
	new: &mut (impl Read + Seek),
	dest: &mut (impl Write + Seek),
	level: Option<i32>,
	old_len_hint: Option<u64>,
	new_len_hint: Option<u64>,
) -> Result<()> {
	let level = level.unwrap_or(3);

	let (num_chunks, old_len, new_len, olf, nlf) =
		calc_chunk_num(old, new, old_len_hint, new_len_hint)?;

	let chunks_o = calc_chunks(num_chunks, olf);
	let chunks_n = calc_chunks(num_chunks, nlf);
	let mut chunks = chunks_o.zip(chunks_n).peekable();

	// write chunk count
	dest.write_all(&(num_chunks as u64).to_be_bytes())?;

	while let Some((co1, cn1)) = chunks.next() {
		let (co2, cn2) = *chunks.peek().unwrap_or(&(old_len, new_len));

		// read dictionary into memory
		let mut dict_chunk = vec![0u8; (co2 - co1) as usize].into_boxed_slice();
		old.seek(SeekFrom::Start(co1))?;
		old.read_exact(&mut dict_chunk)?;
		let dict_chunk = EncoderDictionary::new(&dict_chunk, level); // requires crate `experimental` to elide copy

		// prepare streams
		new.seek(SeekFrom::Start(cn1))?;
		let mut throttled_new = new.take(cn2 - cn1);
		// leave an 8-byte space for the length count
		dest.seek_relative(8)?;
		let mut counting_writer = countio::Counter::new(&mut *dest);

		let mut enc = Encoder::with_prepared_dictionary(&mut counting_writer, &dict_chunk)?;
		enc.long_distance_matching(true)?;
		enc.set_pledged_src_size(Some(cn2 - cn1))?;
		enc.include_checksum(false)?; // we do our own redundancy checks
		enc.include_contentsize(false)?; // not particularly helpful to us

		// run the compression
		std::io::copy(&mut throttled_new, &mut enc)?;
		_ = enc.finish()?;

		let diff_len = counting_writer.writer_bytes();
		// seek back
		dest.seek_relative(-(diff_len as i64) - 8)?;
		// write length
		dest.write_all(&diff_len.to_be_bytes())?;
		// seek forward again
		dest.seek_relative(diff_len as i64)?;
	}

	Ok(())
}

/// Applies a `diff` from `old`, and writes the new file into `dest`.
/// The seek points must be at the beginning of the old file and at the start of the diff structure.
/// `old_len_hint` should either not be provided, or MUST be EXACTLY the size of the old stream, allowing eliding length determination.
/// The number of bytes written to the new file is returned.
pub fn apply(
	old: &mut (impl Read + Seek),
	diff: &mut (impl Read + Seek),
	dest: &mut impl Write,
	old_len_hint: Option<u64>,
) -> Result<u64> {
	let old_len = resolve_len(old, old_len_hint)?;

	// read number of chunks
	let num_chunks = read_u64(diff)?;

	let mut chunks = calc_chunks(num_chunks as f64, old_len as f64).peekable();

	let mut written = 0u64;

	while let Some(co1) = chunks.next() {
		let co2 = *chunks.peek().unwrap_or(&old_len);

		// read dictionary into memory
		let mut dict_chunk = vec![0u8; (co2 - co1) as usize].into_boxed_slice();
		old.seek(SeekFrom::Start(co1))?;
		old.read_exact(&mut dict_chunk)?;
		let dict_chunk = DecoderDictionary::new(&dict_chunk); // requires crate `experimental` to elide copy

		// read length of compressed blob & setup streams
		let diff_c_len = read_u64(diff)?;
		//diff.seek(SeekFrom::Start(cn1))?;
		let throttled_diff = BufReader::new(diff.take(diff_c_len));

		let mut counter = countio::Counter::new(&mut *dest);

		// decompress diff
		let mut decoder = Decoder::with_prepared_dictionary(throttled_diff, &dict_chunk)?;
		std::io::copy(&mut decoder, &mut counter)?;

		written += counter.writer_bytes() as u64;
	}

	Ok(written)
}

#[cfg(test)]
mod tests {
	use crate::zstddiff::{apply, diff};
	use rand::random;
	use std::io::Seek;

	#[test]
	fn test_zstddiff_small() {
		// 64k
		let mut data_old = vec![0u8; 64_000];
		// set some bits
		for _ in 0..128_000 {
			let oset = (random::<f64>() * data_old.len() as f64) as usize;
			data_old[oset] = random();
		}
		// change it a bit
		let mut data_new = data_old.repeat(2);
		for _ in 0..16_000 {
			let oset = (random::<f64>() * data_new.len() as f64) as usize;
			data_new[oset] = random();
		}

		// diff it!
		let mut diff_cursor = std::io::Cursor::new(Vec::new());

		// re-borrow to kill the cursor's ability to resize
		let mut old_reader = std::io::Cursor::new(&*data_old);
		let mut new_reader = std::io::Cursor::new(&mut *data_new);

		diff(
			&mut old_reader,
			&mut new_reader,
			&mut diff_cursor,
			None,
			Some(64_000),
			None,
		)
		.unwrap();

		// now we have a diff, let's apply it
		let mut final_writer = std::io::Cursor::new(Vec::new());
		old_reader.rewind().unwrap();
		diff_cursor.rewind().unwrap();

		let dcsz = apply(&mut old_reader, &mut diff_cursor, &mut final_writer, None).unwrap();

		// check if everything is ok
		assert_eq!(dcsz, 128_000);
		assert_eq!(*data_new, *final_writer.into_inner());
	}
}
