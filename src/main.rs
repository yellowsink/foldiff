use anyhow::{bail, ensure, Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

mod foldiff;
mod zstddiff;
mod hash;

#[derive(Parser, Debug)]
#[command(version = "2023-09-06.r1", about)]
struct Cli {
	#[command(subcommand)]
	command: Commands,
	/// Overwrite the output path if it exists
	#[arg(short, long, default_value_t = false)]
	force: bool
}

#[derive(Subcommand, Debug)]
enum Commands {
	/// Create a diff from two similar folders
	Diff {
		/// Path to the source / "old" folder
		old: String,
		/// Path to the "new" folder
		new: String,
		/// Path to where to create the diff file
		diff: String,
	},
	/// Apply a diff to a folder
	Apply {
		/// Path to the source / "old" folder
		old: String,
		/// Path to the diff file
		diff: String,
		/// Path to where to create the "new" folder
		new: String,
	}
}

fn main() -> Result<()> {
	let cli = Cli::parse();

	match &cli.command {
		Commands::Diff { diff, new, old } => {
			let old_root: PathBuf = old.into();
			let new_root: PathBuf = new.into();
			// check both exist
			ensure!(std::fs::metadata(&old_root).context("old path must exist")?.is_dir(), "old path must be a directory");
			ensure!(std::fs::metadata(&new_root).context("new path must exist")?.is_dir(), "new path must be a directory");

			// check for diff file existence and possibly delete it
			if std::fs::exists(diff).context("Failed to check for output existence")? {
				let meta = std::fs::symlink_metadata(diff).context("Failed to check existing output file type")?;
				if meta.is_dir() {
					bail!("Output diff file exists but is a directory");
				}
				else {
					ensure!(!meta.is_symlink(), "Output diff file exists but is a symlink");
				}

				if !cli.force {
					// check first!
					let cont = dialoguer::Confirm::new()
						.with_prompt("Output diff file exists, overwrite it?")
						.interact()?;

					if !cont { bail!("Output diff file already exists"); }
				}

				std::fs::remove_file(diff)?;
			}


			// scan the file system
			let diff_state = foldiff::DiffingDiff::scan(old_root, new_root)?;
			println!("{diff_state:?}");

			// emit the diff to disk
			diff_state.write_to_file(Path::new(diff))?;

		}
		Commands::Apply { .. } => {
			todo!()
		}
	}

	Ok(())
}
