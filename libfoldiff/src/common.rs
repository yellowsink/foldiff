use std::fs::File;
use std::path::Path;

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