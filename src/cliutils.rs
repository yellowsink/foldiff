use anyhow::Result;
use dialoguer::Confirm;
use indicatif::{ProgressBar, ProgressStyle};
use std::sync::LazyLock;

pub fn confirm(msg: &str) -> Result<bool> {
	Ok(Confirm::new().with_prompt(msg).interact()?)
}

static SPINNER_TEMPLATE_COUNT: &str = "{spinner} [{pos}] {msg}";
static SPINNER_TEMPLATE_SIMPLE: &str = "{spinner} {msg}";
static SPINNER_TICKS: &[&str] = &["⠙","⠸","⠴","⠦","⠇","⠋","✓"];
// ours:   "⠙⠸⠴⠦⠇⠋✓"
// default: "⠁⠁⠉⠙⠚⠒⠂⠂⠒⠲⠴⠤⠄⠄⠤⠠⠠⠤⠦⠖⠒⠐⠐⠒⠓⠋⠉⠈⠈ "

static PROGRESS_TEMPLATE: &str = "{spinner} {msg} [{pos}/{len}] {wide_bar}";
static PROGRESS_TICKS: &[&str] = &[" ", "✓"];

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
	ProgressStyle::with_template(PROGRESS_TEMPLATE).unwrap().tick_strings(PROGRESS_TICKS)
});

static PROGRESS_STYLE_FINISHED: LazyLock<ProgressStyle> = LazyLock::new(|| {
	ProgressStyle::with_template(
		&console::style(PROGRESS_TEMPLATE).green().to_string()
	).unwrap().tick_strings(PROGRESS_TICKS)
});

pub fn create_spinner(msg: &str, count: bool) -> ProgressBar {
	ProgressBar::new_spinner().with_message(msg.to_string()).with_style(
		if count { SPINNER_STYLE_COUNT.clone() } else { SPINNER_STYLE_SIMPLE.clone() }
	)
}

pub fn finish_spinner(s: &ProgressBar, count: bool) {
	s.set_style(
		if count { SPINNER_STYLE_FINISHED_COUNT.clone() } else { SPINNER_STYLE_FINISHED_SIMPLE.clone() }
	);
	s.abandon();
}

pub fn create_bar(msg: &str, len: u64) -> ProgressBar {
	ProgressBar::new(len).with_message(msg.to_string()).with_style(PROGRESS_STYLE.clone())
}

pub fn finish_bar(b: &ProgressBar) {
	b.set_style(PROGRESS_STYLE_FINISHED.clone());
	b.abandon();
}