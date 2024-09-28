use crate::manifest::DiffManifest;
use crate::common::{MAGIC_BYTES, VERSION_NUMBER_1_1_0, VERSION_NUMBER_1_0_0_R, VERSION_NUMBER_LATEST};
use anyhow::{bail, Context, Result};
use std::io::{Read, Seek, Write};
use zstd::Encoder;
use crate::reporting::{AutoSpin, Reporter};

// 1.0.0-r to v1.1.0
fn upgrade_100r_110<TSpin: Reporter+Sync>(mut src: impl Read+Seek, mut dst: impl Write+Seek) -> Result<()> {
	let s = TSpin::new("Upgrading from FLDF 1.0.0-r to FLDF 1.1.0");
	let s = AutoSpin::spin(&s);
	
	// write magic bytes and version number to dst
	dst.write_all(&MAGIC_BYTES).context("Failed to write to destination file")?;
	dst.write_all(&VERSION_NUMBER_1_1_0)?;
	
	// get size of manifest by reading through it (this is inefficient but old formats be like that)
	let pre_seek = src.stream_position()?;
	// yes, we just throw this away. it's inefficient to re-serialize it.
	DiffManifest::read_100r(&mut src)?;
	let manifest_size = src.stream_position()? - pre_seek;
	
	src.seek_relative(-(manifest_size as i64))?;
	
	// compress manifest
	dst.write_all(&[0u8; 8])?;
	let mut cw = countio::Counter::new(&mut dst);
	let mut enc = Encoder::new(&mut cw, 19)?.auto_finish();
	std::io::copy(&mut (&mut src).take(manifest_size), &mut enc)?;
	drop(enc);
	
	let comp_size = cw.writer_bytes();
	dst.seek_relative(-(comp_size as i64) - 8)?;
	dst.write_all(&comp_size.to_be_bytes())?;
	dst.seek_relative(comp_size as i64)?;
	
	// copy the rest of the data over (blobs)
	std::io::copy(&mut src, &mut dst)?;
	
	s.all_good();
	Ok(())
}

pub fn auto_upgrade<TSpin: Reporter+Sync>(mut src: impl Read+Seek, dst: impl Write+Seek) -> Result<()> {
	let ver = DiffManifest::verify_and_read_ver(&mut src)?;
	
	match ver {
		VERSION_NUMBER_LATEST => bail!("Diff is up to date! (FLDF v{}.{}.{})", ver[1], ver[2], ver[3]),
		VERSION_NUMBER_1_0_0_R => upgrade_100r_110::<TSpin>(src, dst),
		_ => unreachable!(),
	}
}