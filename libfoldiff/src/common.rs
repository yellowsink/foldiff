use std::fs::File;
use std::path::Path;
use anyhow::Context;
use crate::hash;

pub const MAGIC_BYTES: [u8; 4] = *b"FLDF";
pub const VERSION_NUMBER_1_0_0_R: [u8; 4] = [1, 0, 0, b'r']; // v1.0.0-r
pub const VERSION_NUMBER_1_1_0: [u8; 4] = [0, 1, 1, 0]; // v1.1.0
pub const VERSION_NUMBER_LATEST: [u8; 4] = VERSION_NUMBER_1_1_0;

/// internal configuration struct passed into foldiff to control its operation
#[derive(Copy, Clone, Debug)]
pub struct FoldiffCfg {
	pub threads: usize,
	pub level_new: u8,
	pub level_diff: u8,
}

/// creates a file and all necessary parent directories
pub fn create_file(p: &Path) -> std::io::Result<File> {
	if let Some(p) = p.parent() {
		std::fs::create_dir_all(p)?;
	}
	File::create(p)
}

// Reflinks or copies a file and hashes it
pub fn copy_rl_hash(src_p: impl AsRef<Path>, dst_p: impl AsRef<Path>) -> anyhow::Result<u64> {
	let src_p = src_p.as_ref();
	let dst_p = dst_p.as_ref();
	
	// if we're on *nix, try reflinking
	if cfg!(unix) && reflink::reflink(&src_p, &dst_p).is_ok() {
		// reflinked, check the hash
		hash::hash_file(&src_p).context(format!("Failed to hash file copied from {src_p:?}"))
	}
	else {
		// reflink failed or we're on windows, copy
		// copying in kernel space would be slightly faster but we have to check the hash
		let mut src = File::open(&src_p).context(format!("Failed to open file to copy from {src_p:?}"))?;
		let mut dst = create_file(&dst_p).context(format!("Failed to create file to copy to {dst_p:?}"))?;

		let mut hw = hash::XXHashStreamer::new(&mut dst);
		std::io::copy(&mut src, &mut hw).context(format!("Failed to copy file {src_p:?}"))?;

		Ok(hw.finish())
	}
}

pub fn copy_rl(src_p: impl AsRef<Path>, dst_p: impl AsRef<Path>) -> std::io::Result<()> {
	let src_p = src_p.as_ref();
	let dst_p = dst_p.as_ref();
	
	// if we're on *nix, try reflinking
	if cfg!(unix) && reflink::reflink(&src_p, &dst_p).is_ok() {
		Ok(())
	}
	else {
		std::fs::copy(src_p, dst_p).map(|_| ())
	}
}

/// If a vec is empty, do nothing. If it contains some errors, aggregate and return them.
#[macro_export]
macro_rules! aggregate_errors {
	($e:expr) => {{
		let e = $e;
		if !e.is_empty() {
			anyhow::bail!("Failed with multiple errors:\n{}", e.into_iter().map(|e| format!("{e}")).collect::<Vec<_>>().join("\n"));
		}
	}};
}