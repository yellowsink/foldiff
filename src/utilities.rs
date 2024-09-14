use std::fs::File;
use std::path::Path;

// threading error handling utils

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
			throw_err_async!($errs, anyhow::anyhow!(format!("{e:?}")).context(format!($fmt, $($($arg)*)?)));
		}
		else {
			v.unwrap()
		}
	}};
}

/// unwraps res and, if it's an error, returns Some(err)
#[macro_export]
macro_rules! handle_res_parit {
	($res:expr, $fmt:expr $(, $($arg:tt)+)?) => {{
		let v = $res;
		if let Err(e) = v {
			return Some(anyhow!(format!("{e:?}")).context(format!($fmt, $($($arg)*)?)));
		}
		else {
			v.unwrap()
		}
	}};
	($res:expr) => {{
		let v = $res;
		if let Err(e) = v {
			return Some(anyhow!(format!("{e:?}")));
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