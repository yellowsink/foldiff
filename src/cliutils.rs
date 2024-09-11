use std::sync::LazyLock;
use dialoguer::Confirm;
use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};

pub fn confirm(msg: &str) -> Result<bool> {
	Ok(Confirm::new().with_prompt(msg).interact()?)
}

static SPINNER_TEMPLATE: &str = "{spinner} [{pos} entries] {msg}";
static SPINNER_TICKS: &[&str] = &["⠙","⠸","⠴","⠦","⠇","⠋","✓"];
// ours:   "⠙⠸⠴⠦⠇⠋✓"
// default: "⠁⠁⠉⠙⠚⠒⠂⠂⠒⠲⠴⠤⠄⠄⠤⠠⠠⠤⠦⠖⠒⠐⠐⠒⠓⠋⠉⠈⠈ "

static PROGRESS_STYLE: LazyLock<ProgressStyle> = LazyLock::new(|| {
	ProgressStyle::with_template(SPINNER_TEMPLATE).unwrap().tick_strings(SPINNER_TICKS)
});

static PROGRESS_STYLE_FINISHED: LazyLock<ProgressStyle> = LazyLock::new(|| {
	ProgressStyle::with_template(
		&console::style(SPINNER_TEMPLATE).green().to_string()
	).unwrap().tick_strings(SPINNER_TICKS)
});

pub fn create_spinner(msg: &str) -> ProgressBar {
	ProgressBar::new_spinner().with_message(msg.to_string()).with_style(PROGRESS_STYLE.clone())
}

pub fn finish_spinner(s: &ProgressBar) {
	s.set_style(PROGRESS_STYLE_FINISHED.clone());
	s.abandon();
}