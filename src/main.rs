use std::fs::File;
use anyhow::{bail, ensure, Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use crate::foldiff::{ApplyingDiff, DiffManifest, DiffingDiff, FldfCfg};

mod foldiff;
mod zstddiff;
mod hash;
mod cliutils;
mod utilities;
mod verify;

fn fetch_logical_procs() -> u32 {
	num_cpus::get() as u32
}

#[derive(Parser, Debug)]
#[command(
	version = "v1.2.0",
	about,
	long_version = "v1.2.0
   writing fldf v1.1.0
   reading fldf 1.0.0-r, v1.1.0"
)]
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
	},
	/// Check that two folders are identical, or that they match a given diff file
	Verify {
		/// Path to the source / "old" folder
		old: String,
		/// Path to the "new" folder
		new: String,
		/// If supplied, the path to the diff to verify against. If not supplied, just checks if the folders are identical
		diff: Option<String>
	}
}

fn main() -> Result<()> {
	// attach debugger
	//cliutils::confirm("")?;

	let cli = Cli::parse();

	let threads =
		if cli.threads == 0 {
			fetch_logical_procs()
		}
		else {
			cli.threads
		};

	rayon::ThreadPoolBuilder::new()
		.num_threads(threads as usize)
		.build_global()?;

	match &cli.command {
		Commands::Diff { diff, new, old, level_diff, level_new } => {
			let cfg = FldfCfg {
				threads,
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
			let mut diff_state = DiffingDiff::scan(old_root, new_root)?;
			//println!("{diff_state:?}");

			// emit the diff to disk
			diff_state.write_to_file(Path::new(diff), &cfg)?;

		}
		Commands::Apply { old, diff, new } => {
			let old_root: PathBuf = old.into();
			let new_root: PathBuf = new.into();
			// check existence
			ensure!(std::fs::metadata(&old_root).context("old path must exist")?.is_dir(), "old path must be a directory");
			ensure!(std::fs::metadata(diff).context("diff must exist")?.is_file(), "diff must be a file");

			// check for out folder existence and possibly delete it
			if std::fs::exists(&new_root).context("Failed to check for output existence")? {
				if !cli.force {
					// check first!
					let cont = cliutils::confirm("Output folder exists, overwrite it?")?;

					if !cont { bail!("Output folder already exists"); }
				}

				std::fs::remove_dir_all(new).context("Failed to remove folder")?;
			}

			let mut diff_state = ApplyingDiff::read_from_file(&PathBuf::from(diff))?;
			diff_state.apply(old_root, new_root)?;
		},
		Commands::Verify { new, old, diff } => {
			if let Some(diff) = diff {
				let f = File::open(diff).context("Failed to open diff file to verify with")?;
				let manifest = DiffManifest::read_from(f).context("Failed to read diff file to verify with")?;
				verify::verify(Path::new(old), Path::new(new), &manifest)?;
			}
			else {
				verify::test_equality(Path::new(old), Path::new(new))?;
			}
		},
	}

	Ok(())
}
