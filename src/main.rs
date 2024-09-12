use anyhow::{bail, ensure, Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use crate::foldiff::FldfCfg;

mod foldiff;
mod zstddiff;
mod hash;
mod cliutils;

fn fetch_logical_procs() -> u32 {
	num_cpus::get() as u32
}

#[derive(Parser, Debug)]
#[command(version = "2023-09-06.r1", about)]
struct Cli {
	#[command(subcommand)]
	command: Commands,
	/// Overwrite the output path if it exists
	#[arg(short, long, default_value_t = false)]
	force: bool,
	/// How many threads to use ("-T 0" = number of logical processors)
	#[arg(short = 'T', long, default_value_t = 0)]
	threads: u32
}

// picking the default value for -Z:
// 7GB tar, 4C8T
// | l# | ts  | r%   |
// | 3  | 30  | 89.6 |
// | 5  | 45  | 89.3 |
// | 7  | 58  | 88.9 |
// | 10 | 108 | 87.3 |
// | 15 | 243 | 87.2 |
// conclusion: -Z7
// 3 is totally fine for -D because patching happens to work really well even with super low levels

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
		/// Zstd compression level to use for compressing new files (1 = weakest, 19 = strongest)
		#[arg(short = 'Z', long, default_value_t = 7)]
		level_new: u8,
		/// Zstd compression level to use for diffing (1 = weakest, 19 = strongest)
		#[arg(short = 'D', long, default_value_t = 3)]
		level_diff: u8
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
	// attach debugger
	//cliutils::confirm("")?;

	let cli = Cli::parse();

	let num_threads =
		if cli.threads == 0 {
			fetch_logical_procs()
		}
		else {
			cli.threads
		};
	
	match &cli.command {
		Commands::Diff { diff, new, old, level_diff, level_new } => {
			let cfg = FldfCfg {
				threads: num_threads,
				level_new: *level_new,
				level_diff: *level_diff
			};
			
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
					let cont = cliutils::confirm("Output diff file exists, overwrite it?")?;

					if !cont { bail!("Output diff file already exists"); }
				}

				std::fs::remove_file(diff).context("Failed to remove file")?;
			}

			// scan the file system
			let mut diff_state = foldiff::DiffingDiff::scan(old_root, new_root)?;
			//println!("{diff_state:?}");

			// emit the diff to disk
			diff_state.write_to_file(Path::new(diff))?;

		}
		Commands::Apply { .. } => {
			let cfg = FldfCfg {
				threads: num_threads,
				// levels are irrelevant
				level_new: 0,
				level_diff: 0
			};
			
			todo!()
		}
	}

	Ok(())
}
