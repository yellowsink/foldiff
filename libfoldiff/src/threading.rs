

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

/// Sets the number of threads in the global thread pool.
/// Must be called before any tasks are run in it.
pub fn set_num_threads(thr: usize) -> Result<(), impl std::error::Error> {
	rayon::ThreadPoolBuilder::new()
		.num_threads(thr)
		.build_global()
}