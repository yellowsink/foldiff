use std::fs::File;
use std::hash::Hasher;
use std::io::{Read, Write};
use camino::Utf8Path;
use twox_hash::XxHash64;

#[derive(Clone, Default)]
pub struct XXHasher(XxHash64);

impl Write for XXHasher {
	fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
		self.0.write(buf);
		Ok(buf.len())
	}

	fn flush(&mut self) -> std::io::Result<()> {
		Ok(())
	}
}

impl XXHasher {
	fn finish(&self) -> u64 {
		self.0.finish()
	}
}

/*pub fn hash(data: &[u8]) -> u64 {
	let mut h = Hasher::default();
	h.write_all(data).unwrap();
	h.finish()
}*/

pub fn hash_stream(s: &mut impl Read) -> std::io::Result<u64> {
	let mut h = XXHasher::default();
	std::io::copy(s, &mut h)?;
	Ok(h.finish())
}

pub fn hash_file(p: &Utf8Path) -> anyhow::Result<u64> {
	Ok(hash_stream(&mut File::open(p)?)?)
}

pub struct XXHashStreamer<S>(XXHasher, S);

impl<S> XXHashStreamer<S> {
	pub fn new(w: S) -> Self {
		Self(XXHasher::default(), w)
	}

	pub fn finish(&self) -> u64 {
		self.0.finish()
	}
}

impl<W: Write> Write for XXHashStreamer<W> {
	fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
		let written = self.1.write(buf)?;
		_ = self.0.write(&buf[0..written]).unwrap(); // infallible
		Ok(written)
	}
	fn flush(&mut self) -> std::io::Result<()> {
		self.1.flush()
	}
}

impl<R: Read> Read for XXHashStreamer<R> {
	fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
		let res = self.1.read(buf);
		if let Ok(b) = res {
			_ = self.0.write(&buf[0..b]).unwrap();
		}
		res
	}
}

#[cfg(test)]
mod tests {
	use std::io::Seek;
	use super::*;
	use tempfile::tempfile;

	#[test]
	fn test_hash_streamer() {
		// create tmp file
		let mut f = tempfile().unwrap();
		let mut hs = XXHashStreamer::new(&mut f);

		// write random stuff to it
		for _ in 0..1_000 {
			let buf = [0u8; 64];
			hs.write_all(&buf).unwrap();
		}

		let hash_hs_write = hs.finish();

		f.rewind().unwrap();

		let mut hs = XXHashStreamer::new(&mut f);
		// read it all
		std::io::copy(&mut hs, &mut std::io::sink()).unwrap();

		let hash_hs_read = hs.finish();

		f.rewind().unwrap();

		let hash_real = hash_stream(&mut f).unwrap();

		assert_eq!(hash_real, hash_hs_write);
		assert_eq!(hash_real, hash_hs_read);
	}
}