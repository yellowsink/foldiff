// threading error handling utils used in ApplyingDiff::apply()

use std::io::{IoSliceMut, Read, Seek, SeekFrom};
use std::sync::Mutex;

/// Adds err to errs and returns
#[macro_export]
macro_rules! throw_err_async {
	($errs:expr, $err:expr) => {
		if let Ok(v) = &mut $errs.lock() {
			v.push($err);
		}
		return;
	};
}

/// Unwraps res and if its an error, adds it to errs, adds the given context format, and returns
#[macro_export]
macro_rules! handle_res_async {
	($errs:expr, $res:expr, $fmt:expr $(, $($arg:tt)+)?) => {{
		let v = $res;
		if let Err(e) = v {
			throw_err_async!($errs, anyhow!(format!("{e:?}")).context(format!($fmt, $($($arg)*)?)));
		}
		else {
			v.unwrap()
		}
	}};
}

/// Brokers out read+seek access to a reader
pub struct ReadSeekBroker<R: Read+Seek>(Mutex<(R, u64)>);

pub struct RSBReader<'a, R: Read+Seek> {
	id: u64,
	seek: u64,
	broker: &'a ReadSeekBroker<R>
}

impl<R: Read+Seek> ReadSeekBroker<R> {
	pub fn new(r: R) -> Self {
		Self(Mutex::new((r, 0)))
	}
	
	pub fn create_reader(&self) -> RSBReader<R> {
		RSBReader {
			id: rand::random(),
			seek: 0,
			broker: self
		}
	}
}

macro_rules! wrap_seek {
	($self:expr, $name:tt $(, $arg:expr)?) => {{
		// get ownership from the broker
		let mut l = $self.broker.0.lock().expect(concat!("Couldn't lock mutex in RSBReader::", stringify!($name)));
		// invalidate the "we don't need to seek" optimisation
		l.1 = 0;
		// operation
		l.0.$name($($arg)?)
	}};
}

macro_rules! wrap_read {
	($self:expr, $name:tt, $arg:expr) => {{
		// get ownership from the broker
		let mut read_and_id = $self.broker.0.lock().expect(concat!("Couldn't lock mutex in RSBReader::", stringify!($name)));

		// last reader was NOT us → seek (checking this is an optimisation to reduce seeks)
		if read_and_id.1 != $self.id {
			read_and_id.0.seek(SeekFrom::Start($self.seek))?;
		}
		// mark our territory
		read_and_id.1 = $self.id;

		// perform read
		read_and_id.0.$name($arg)?
	}};
}

impl<'a, R: Read+Seek> Seek for RSBReader<'a, R> {
	fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
		wrap_seek!(self, seek, pos)
	}

	fn rewind(&mut self) -> std::io::Result<()> {
		wrap_seek!(self, rewind)
	}

	fn stream_position(&mut self) -> std::io::Result<u64> {
		wrap_seek!(self, stream_position)
	}

	fn seek_relative(&mut self, offset: i64) -> std::io::Result<()> {
		wrap_seek!(self, seek_relative, offset)
	}
}

impl<'a, R: Read+Seek> Read for RSBReader<'a, R> {
	fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
		let bytes_read = wrap_read!(self, read, buf);
		self.seek += bytes_read as u64;
		Ok(bytes_read)
	}

	fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> std::io::Result<usize> {
		let bytes_read = wrap_read!(self, read_vectored, bufs);
		self.seek += bytes_read as u64;
		Ok(bytes_read)
	}

	fn read_to_end(&mut self, buf: &mut Vec<u8>) -> std::io::Result<usize> {
		let bytes_read = wrap_read!(self, read_to_end, buf);
		self.seek += bytes_read as u64;
		Ok(bytes_read)
	}

	fn read_to_string(&mut self, buf: &mut String) -> std::io::Result<usize> {
		let bytes_read = wrap_read!(self, read_to_string, buf);
		self.seek += bytes_read as u64;
		Ok(bytes_read)
	}

	fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<()> {
		wrap_read!(self, read_exact, buf);
		self.seek += buf.len() as u64;
		Ok(())
	}
}