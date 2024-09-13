// threading error handling utils used in ApplyingDiff::apply()

use std::fs::File;
use std::path::Path;

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

/// creates a file and all necessary parent directories
pub fn create_file(p: &Path) -> std::io::Result<File> {
	if let Some(p) = p.parent() {
		std::fs::create_dir_all(p)?;
	}
	File::create(p)
}
