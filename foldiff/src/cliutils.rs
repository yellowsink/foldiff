use anyhow::Result;
use dialoguer::Confirm;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::sync::LazyLock;
use libfoldiff::reporting::{CanBeWrappedBy, Reporter, ReporterSized, ReportingMultiWrapper};

pub fn confirm(msg: &str) -> Result<bool> {
	Ok(Confirm::new().with_prompt(msg).interact()?)
}

static SPINNER_TEMPLATE_COUNT: &str = "{spinner} [{pos}] {msg}";
static SPINNER_TEMPLATE_SIMPLE: &str = "{spinner} {msg}";
static SPINNER_TICKS: &[&str] = &["⠙","⠸","⢰","⣠","⣄","⡆","⠇","⠋","✓"];
// default: "⠁⠁⠉⠙⠚⠒⠂⠂⠒⠲⠴⠤⠄⠄⠤⠠⠠⠤⠦⠖⠒⠐⠐⠒⠓⠋⠉⠈⠈ "

static PROGRESS_TEMPLATE: &str = "{spinner} [{percent:>3}% {pos:>3}/{len:3}] {msg} {wide_bar}";
static PROGRESS_TEMPLATE_FINISHED: &str = "{spinner} [{percent:>3}% {pos:>3}/{len:3}] {msg}";
//static PROGRESS_TICKS: &[&str] = &[" ", "✓"];

static SPINNER_STYLE_COUNT: LazyLock<ProgressStyle> = LazyLock::new(|| {
	ProgressStyle::with_template(SPINNER_TEMPLATE_COUNT).unwrap().tick_strings(SPINNER_TICKS)
});
static SPINNER_STYLE_SIMPLE: LazyLock<ProgressStyle> = LazyLock::new(|| {
	ProgressStyle::with_template(SPINNER_TEMPLATE_SIMPLE).unwrap().tick_strings(SPINNER_TICKS)
});

static SPINNER_STYLE_FINISHED_COUNT: LazyLock<ProgressStyle> = LazyLock::new(|| {
	ProgressStyle::with_template(
		&console::style(SPINNER_TEMPLATE_COUNT).green().to_string()
	).unwrap().tick_strings(SPINNER_TICKS)
});
static SPINNER_STYLE_FINISHED_SIMPLE: LazyLock<ProgressStyle> = LazyLock::new(|| {
	ProgressStyle::with_template(
		&console::style(SPINNER_TEMPLATE_SIMPLE).green().to_string()
	).unwrap().tick_strings(SPINNER_TICKS)
});

static PROGRESS_STYLE: LazyLock<ProgressStyle> = LazyLock::new(|| {
	ProgressStyle::with_template(PROGRESS_TEMPLATE).unwrap().tick_strings(SPINNER_TICKS)
});

static PROGRESS_STYLE_FINISHED: LazyLock<ProgressStyle> = LazyLock::new(|| {
	ProgressStyle::with_template(
		&console::style(PROGRESS_TEMPLATE_FINISHED).green().to_string()
	).unwrap().tick_strings(SPINNER_TICKS)
});

// implement libfoldiff::reporting for indicatif

pub struct Spinner<const COUNT: bool>(ProgressBar);

impl<const COUNT: bool> Reporter for Spinner<COUNT> {
	fn new(msg: &str) -> Self {
		Self(ProgressBar::new_spinner()
			.with_message(msg.to_string())
			.with_style(
				if COUNT { SPINNER_STYLE_COUNT.clone() } else { SPINNER_STYLE_SIMPLE.clone() }
			))
	}

	fn incr(&self, n: usize) {
		// TODO: retain normal steady-tick implementation of incr() not ticking it
		self.0.inc(n as u64);
	}

	fn count(&self) -> usize {
		self.0.position() as usize
	}

	fn tick(&self) {
		self.0.tick();
	}

	fn done_clear(&self) {
		self.0.finish_and_clear();
	}

	fn done(&self) {
		self.0.set_style(
			if COUNT { SPINNER_STYLE_FINISHED_COUNT.clone() } else { SPINNER_STYLE_FINISHED_SIMPLE.clone() }
		);
		self.0.abandon();
	}

	fn suspend<F: FnOnce() -> R, R>(&self, f: F) -> R {
		self.0.suspend(f)
	}
}

pub struct Bar(ProgressBar);

impl Reporter for Bar {
	fn new(msg: &str) -> Self {
		Self(ProgressBar::new(0)
			.with_message(msg.to_string())
			.with_style(PROGRESS_STYLE.clone()))
	}

	fn incr(&self, n: usize) {
		self.0.inc(n as u64);
	}

	fn count(&self) -> usize {
		self.0.position() as usize
	}

	fn tick(&self) {
		self.0.tick();
	}

	fn done_clear(&self) {
		self.0.finish_and_clear();
	}

	fn done(&self) {
		self.0.set_style(PROGRESS_STYLE_FINISHED.clone());
		self.0.abandon();
	}

	fn suspend<F: FnOnce() -> R, R>(&self, f: F) -> R {
		self.0.suspend(f)
	}
}

impl ReporterSized for Bar {
	fn new(msg: &str, len: usize) -> Self {
		Self(ProgressBar::new(len as u64)
			.with_message(msg.to_string())
			.with_style(PROGRESS_STYLE.clone()))
	}

	fn set_len(&self, len: usize) {
		self.0.set_length(len as u64);
	}

	fn length(&self) -> usize {
		self.0.length().unwrap() as usize
	}
}

// this is really unnecessary but ok rust, sure, foreign trait implementation rules
pub struct MultiWrapper(MultiProgress);

impl ReportingMultiWrapper for MultiWrapper {
	fn new() -> Self {
		MultiWrapper(MultiProgress::new())
	}
	
	fn suspend<F: FnOnce() -> R, R>(&self, f: F) -> R {
		self.0.suspend(f)
	}
}

impl<const COUNT: bool> CanBeWrappedBy<MultiWrapper> for Spinner<COUNT> {
	fn add_to(self, w: &MultiWrapper) -> Self {
		Spinner(w.0.add(self.0))
	}
}

impl CanBeWrappedBy<MultiWrapper> for Bar {
	fn add_to(self, w: &MultiWrapper) -> Self {
		Bar(w.0.add(self.0))
	}
}