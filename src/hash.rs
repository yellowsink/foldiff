use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use twox_hash::Xxh3Hash64;

#[derive(Clone, Default)]
pub struct Hasher(Xxh3Hash64);

impl Write for Hasher {
	fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
		std::hash::Hasher::write(&mut self.0, buf);
		Ok(buf.len())
	}

	fn flush(&mut self) -> std::io::Result<()> {
		Ok(())
	}
}

impl Hasher {
	fn finish(&self) -> u64 {
		std::hash::Hasher::finish(&self.0)
	}
}

/*pub fn hash(data: &[u8]) -> u64 {
	let mut h = Hasher::default();
	h.write_all(data).unwrap();
	h.finish()
}*/

pub fn hash_stream(s: &mut impl Read) -> std::io::Result<u64> {
	let mut h = Hasher::default();
	std::io::copy(s, &mut h)?;
	Ok(h.finish())
}

pub fn hash_file(p: &Path) -> anyhow::Result<u64> {
	Ok(hash_stream(&mut File::open(p)?)?)
}

pub struct HashStreamer<S>(Hasher, S);

impl<S> HashStreamer<S> {
	pub fn new(w: S) -> Self {
		Self(Hasher::default(), w)
	}

	pub fn finish(&self) -> u64 {
		self.0.finish()
	}
}

impl<W: Write> Write for HashStreamer<W> {
	fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
		_ = self.0.write(buf).unwrap(); // infallible
		self.1.write(buf)
	}
	fn flush(&mut self) -> std::io::Result<()> {
		self.1.flush()
	}
}

impl<R: Read> Read for HashStreamer<R> {
	fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
		let res = self.1.read(buf);
		if let Ok(b) = res {
			self.0.write(&buf[0..b]).unwrap();
		}
		res
	}
}

impl <S: Seek> Seek for HashStreamer<S> {
	fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
		self.1.seek(pos)
	}
}