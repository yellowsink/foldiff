// performs diffing using zstd, similar to the --patch-from cli argument in the zstd cli

use anyhow::Result;
use std::io::{Read, Seek, SeekFrom, Write};
use zstd::dict::EncoderDictionary;
use zstd::Encoder;

fn length_of(stream: &mut impl Seek) -> Result<u64> {
    let current_pos = stream.stream_position()?;
    let length = stream.seek(SeekFrom::End(0))?;
    stream.seek(SeekFrom::Start(current_pos))?;
    Ok(length)
}


/// Creates a diff from `old` to `new`, and writes it into `dest`.
/// The diff structure (number of blobs, (length of blob, blob)[]) will be written into `dest` at the current seek point.
/// `level` is the zstd compression level, higher will give smaller diffs.
/// `old_len_hint` and `new_len_hint` should either not be provided, or MUST be EXACTLY the size of the old and new streams, and allows eliding length determination via SeekFrom::End.
pub fn diff(old: &mut (impl Read+Seek), new: &mut (impl Read+Seek), dest: &mut (impl Write+Seek), level: Option<i32>, old_len_hint: Option<u64>, new_len_hint: Option<u64>) -> Result<()> {

    let old_len = old_len_hint.map_or_else(|| length_of(old), Ok)?;
    let new_len = new_len_hint.map_or_else(|| length_of(new), Ok)?;
    let level = level.unwrap_or(3);

    let num_chunks = (old_len as f64) / ((1 << 31) as f64); // 2 GiB (2_147_483_648 bytes)
    let num_chunks = num_chunks.ceil(); // round up to ensure the chunk size is <=
    // yes, these are floats, that is to resist drift
    let chunk_size_old = old_len as f64 / num_chunks;
    let chunk_size_new = new_len as f64 / num_chunks;

    let num_chunks = num_chunks as u64;

    // calculate chunks
    let chunk_range = (0..num_chunks).map(|i| i as f64);
    let chunks_old = chunk_range.clone().map(move |i| (i * chunk_size_old) as u64);
    let chunks_new = chunk_range.map(move |i| (i * chunk_size_new) as u64);

    let mut chunks = chunks_old.zip(chunks_new).peekable();

    // write chunk count
    dest.write_all(&num_chunks.to_be_bytes())?;

    while let Some((co1, cn1)) = chunks.next() {
        let (co2, cn2) = *chunks.peek().unwrap_or(&(old_len, new_len));

        // read dictionary into memory
        let mut dict_chunk = vec![0u8; (co2-co1) as usize].into_boxed_slice();
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

        // run the compression
        std::io::copy(&mut throttled_new, &mut enc)?;
        _ = enc.finish()?;

        let diff_len = counting_writer.writer_bytes();
        // seek back
        dest.seek_relative(-(diff_len as i64) - 8)?;
        // write length
        dest.write_all(&diff_len.to_be_bytes())?;
        // seek forward again
        dest.seek_relative((diff_len + 8) as i64)?;
    }

    Ok(())
}
