// performs diffing using zstd, similar to the --patch-from cli argument in the zstd cli

use anyhow::Result;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use zstd::{Decoder, Encoder};

// bytes
const CHUNK_SIZE: f64 = ((1u64 << 31)/2) as f64; // 1gb

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
	let num_chunks = l1f / CHUNK_SIZE;
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
	level: Option<u8>,
	threads: Option<usize>,
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
		let mut dict_chunk = vec![0u8; (co2 - co1) as usize];//.into_boxed_slice();
		old.seek(SeekFrom::Start(co1))?;
		old.read_exact(&mut dict_chunk)?;

		// prepare streams
		new.seek(SeekFrom::Start(cn1))?;
		let mut throttled_new = new.take(cn2 - cn1);
		// leave an 8-byte space for the length count
		dest.seek_relative(8)?;
		let mut counting_writer = countio::Counter::new(&mut *dest);

		// the results of running GDB on the zstd cli to figure out why --patch-from and -D differ:
		// we can't use the `with_dictionary` etc functions as those are calling the equivalent of
		// `ZSTD_CCtx_loadDictionary_byReference`, which writes to cctx.localDict (see following):
		//    zstd_compress.c:1287:15
		//    fileio.c:1193:5
		// whereas we want to use the equivalent of
		// `ZSTD_CCtx_refPrefix`, which writes to cctx.prefixDict
		// (see zstd_compress.c:1349:9)
		// commit hash 6d6d3db in case any lines move around
		// basically, we want to use a `ref_prefix`, not a dictionary.

		let mut enc = Encoder::with_ref_prefix(&mut counting_writer, level as i32, &dict_chunk)?;
		enc.long_distance_matching(true)?;
		enc.window_log(31)?; // 2GiB (2^31)
		enc.set_pledged_src_size(Some(cn2 - cn1))?;
		enc.include_dictid(false)?; // not using a trained dictionary
		enc.include_checksum(false)?; // we do our own redundancy checks
		enc.include_contentsize(false)?; // not particularly helpful to us
		if let Some(t) = threads {
			enc.multithread(t as u32)?;
		}
		
		// run the compression
		std::io::copy(&mut throttled_new, &mut enc)?;
		let _ = enc.finish()?;

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
	old: &mut impl Read,
	diff: &mut (impl Read + Seek),
	dest: &mut impl Write,
	old_len: u64,
) -> Result<u64> {
	// read number of chunks
	let num_chunks = read_u64(diff)?;

	let mut chunks = calc_chunks(num_chunks as f64, old_len as f64).peekable();

	let mut written = 0u64;

	while let Some(co1) = chunks.next() {
		let co2 = *chunks.peek().unwrap_or(&old_len);

		// read dictionary into memory
		let mut dict_chunk = vec![0u8; (co2 - co1) as usize].into_boxed_slice();
		//debug_assert_eq!(old.stream_position()?, co1);
		//old.seek(SeekFrom::Start(co1))?;
		old.read_exact(&mut dict_chunk)?;

		// read length of compressed blob & setup streams
		let diff_c_len = read_u64(diff)?;
		//diff.seek(SeekFrom::Start(cn1))?;
		let throttled_diff = BufReader::new(diff.take(diff_c_len));

		let mut counter = countio::Counter::new(&mut *dest);

		// decompress diff
		let mut decoder = Decoder::with_ref_prefix(throttled_diff, &dict_chunk)?;
		decoder.window_log_max(31)?; // else we OOM
		std::io::copy(&mut decoder, &mut counter)?;

		written += counter.writer_bytes() as u64;
	}

	Ok(written)
}

#[cfg(test)]
mod tests {
	use super::*;

	use rand::{random, RngCore};
	use std::fs::{remove_file, File};
	use std::io::{BufReader, BufWriter, Read, Seek, Write};
	use zstd::dict::EncoderDictionary;

	// zstd is entirely ignoring my dictionary so fuck it, let's just test that dictionaries work
	// *at all* with this setup i guess
	#[test]
	fn test_zstd_dict() {
		// dictionary trained on the Cornell Movie Dialog Corpus' `movie_lines.txt`, available at
		// https://www.cs.cornell.edu/~cristian/Chameleons_in_imagined_conversations.html
		// split into chunks of 100 lines with the unix `split` command
		//let dictionary: &[u8; 112_640] = include_bytes!("../test_assets/cornell-movie-dict");
		// screw it, use it raw, works fine lol
		let dictionary = include_bytes!("../test_assets/cornell-movie-lines.txt");
		let dictionary = EncoderDictionary::copy(dictionary, 3);

		// some movie quotes :D
		// should get about 77% ratio raw and 60% ratio with this dictionary.
		let quotes = "I’m going to make him an offer he can’t refuse.\
I do wish we could chat longer, but...I'm having an old friend for dinner. Bye.\
My mama always said, life was like a box a chocolates. You never know what you’re gonna get.\
Look at me. Look at me. I'm the captain now.".as_bytes();

		// compress it the boring way
		let mut target_simple = Vec::new();
		let mut enc = Encoder::new(&mut target_simple, 3).unwrap();
		enc.write_all(quotes).unwrap();
		enc.finish().unwrap();

		// compress it with a dictionary
		let mut target_dict = Vec::new();
		let mut enc = Encoder::with_prepared_dictionary(&mut target_dict, &dictionary).unwrap();
		enc.write_all(quotes).unwrap();
		enc.finish().unwrap();

		// if the dictionary worked, we'll get a different output
		assert_ne!(target_simple, target_dict);
	}

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
			None,
			Some(64_000),
			None,
		)
		.unwrap();

		// now we have a diff, let's apply it
		let mut final_writer = std::io::Cursor::new(Vec::new());
		old_reader.rewind().unwrap();
		diff_cursor.rewind().unwrap();

		let ol = resolve_len(&mut old_reader, None).unwrap();
		let dcsz = apply(&mut old_reader, &mut diff_cursor, &mut final_writer, ol).unwrap();

		// check if everything is ok
		assert_eq!(dcsz, 128_000);
		assert_eq!(*data_new, *final_writer.into_inner());
	}

	#[test]
	fn test_zstddiff_large() {
		// create a file to disk here if one doesnt exist from a previous run
		let mut old_file =
			if let Ok(f) = File::open(".unittest_old_file") {
				f
			}
			else {
				eprintln!("generating an 'old' file...");
				let _ = remove_file(".unittest_new_file");
				let mut file = File::create_new(".unittest_old_file").expect("Failed to create old file for unit test");
				let size = 5 * (1u64 << 30); // 5gib
				let mut written = 0u64;

				// write 512mib at a time
				let buf = &mut vec![0u8; 512 * 1024 * 1024].into_boxed_slice();
				assert_eq!((size as usize) % buf.len(), 0);
				let mut rng = rand::thread_rng();
				while written < size {
					rng.fill_bytes(buf);
					file.write_all(buf).unwrap();
					written += buf.len() as u64;
				}

				assert_eq!(written, size);

				file.rewind().unwrap();

				file
			};

		// now have a slightly different copy of it
		let mut new_file =
			if let Ok(f) = File::open(".unittest_new_file") {
				f
			}
			else {
				eprintln!("generating a 'new' file...");
				let mut file = File::create_new(".unittest_new_file").expect("Failed to create new file for unit test");
				let size = old_file.metadata().unwrap().len();

				// scope to give `file` back
				{
					let mut bold = BufReader::new(&mut old_file);
					let mut bnew = BufWriter::new(&mut file);

					const STRIDE_SIZE: u64 = 1024 * 1024; // 1mb

					assert_eq!(size % STRIDE_SIZE, 0);
					// copy data but change it sometimes
					//std::io::copy(&mut bold, &mut bnew).unwrap();
					for _ in 0..(size / STRIDE_SIZE) {
						let mut buf = [0u8; STRIDE_SIZE as usize];
						bold.read_exact(&mut buf).unwrap();

						// 200 deviations per mb
						for _ in 0..200 {
							buf[(random::<f64>() * (buf.len() as f64)) as usize] = random();
						}

						bnew.write_all(&buf).unwrap();
					}
				}

				file.rewind().unwrap();

				file
			};

		// NOW PERFORM DIFFING :D
		eprintln!("diffing to scratch...");
		if File::open(".unittest_diff_scratch").is_ok() {
			remove_file(".unittest_diff_scratch").unwrap();
		}
		let mut diff_scratch = File::create_new(".unittest_diff_scratch").unwrap();

		let ofl = old_file.metadata().unwrap().len();
		let nfl = new_file.metadata().unwrap().len();
		diff(&mut old_file, &mut new_file, &mut diff_scratch, None, None, Some(ofl), Some(nfl)).expect("dif failed");

		// now apply!
		eprintln!("applying to scratch...");
		old_file.rewind().unwrap();
		diff_scratch.rewind().unwrap();

		if File::open(".unittest_fin_scratch").is_ok() {
			remove_file(".unittest_fin_scratch").unwrap();
		}
		let mut fin_scratch = File::create_new(".unittest_fin_scratch").unwrap();

		let ol = resolve_len(&mut old_file, None).unwrap();
		apply(&mut old_file, &mut diff_scratch, &mut fin_scratch, ol).expect("apply failed");

		// now check equality
		fin_scratch.rewind().unwrap();
		new_file.rewind().unwrap();

		eprintln!("verifying output...");
		loop {
			// read a buffer from both files
			let mut buf1 = [0u8; 64 * 1024];
			let mut buf2 = [0u8; 64 * 1024];

			if new_file.read_exact(&mut buf1).is_err() {
				break; // EOF
			}
			fin_scratch.read_exact(&mut buf2).unwrap();

			assert_eq!(buf1, buf2);
		}

		eprintln!("pass :tada:");
		let _ = remove_file(".unittest_diff_scratch");
		let _ = remove_file(".unittest_fin_scratch");
	}
}
