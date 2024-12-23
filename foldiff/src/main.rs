use std::fs::File;
use anyhow::{bail, ensure, Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use clap::{Parser, Subcommand};
use libfoldiff::FoldiffCfg;
use libfoldiff::manifest::DiffManifest;

mod cliutils;

#[derive(Parser, Debug)]
#[command(
	version = "v1.3.1",
	about,
	long_version = "v1.3.1
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
	threads: usize
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
	},
	/// Upgrade a diff from an old file format to the current version
	Upgrade {
		/// Path to the old diff
		old: String,
		/// Path to the destination location
		new: String,
	}
}

fn main() -> Result<()> {
	// attach debugger
	//cliutils::confirm("")?;

	let cli = Cli::parse();

	let threads =
		if cli.threads == 0 {
			num_cpus::get()
		}
		else {
			cli.threads
		};

	libfoldiff::set_num_threads(threads)?;

	match &cli.command {
		Commands::Diff { diff, new, old, level_diff, level_new } => {
			let cfg = FoldiffCfg {
				threads,
				level_new: *level_new,
				level_diff: *level_diff
			};

			let old_root: Utf8PathBuf = old.into();
			let new_root: Utf8PathBuf = new.into();
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
			let mut diff_state = libfoldiff::diffing::scan_to_diff::<cliutils::Spinner<true>>(old_root, new_root)?;
			//println!("{diff_state:?}");

			// emit the diff to disk
			diff_state.write_to_file::<cliutils::Bar, cliutils::Spinner<false>>(Utf8Path::new(diff), &cfg)?;

		}
		Commands::Apply { old, diff, new } => {
			let old_root: Utf8PathBuf = old.into();
			let new_root: Utf8PathBuf = new.into();
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

			let mut diff_state = libfoldiff::applying::read_diff_from_file(&Utf8PathBuf::from(diff))?;
			diff_state.apply::<
				cliutils::MultiWrapper,
				cliutils::Spinner<false>,
				cliutils::Bar
			>(old_root, new_root)?;
		},
		Commands::Verify { new, old, diff } => {
			if let Some(diff) = diff {
				let f = File::open(diff).context("Failed to open diff file to verify with")?;
				let manifest = DiffManifest::read_from(f).context("Failed to read diff file to verify with")?;
				libfoldiff::verify::verify_against_diff::<cliutils::Spinner<true>>(old.as_str().into(), new.as_str().into(), &manifest)?;
			}
			else {
				libfoldiff::verify::test_dir_equality::<cliutils::Spinner<true>>(old.as_str().into(), new.as_str().into())?;
			}
		},
		Commands::Upgrade { new, old } => {
			if std::fs::exists(new).context("Failed to check for destination existence")? {
				if !cli.force {
					let cont = cliutils::confirm("Destination file exists, overwrite it?")?;

					if !cont {
						bail!("Destination file already exists");
					}
				}

				std::fs::remove_file(new).context("Failed to remove file")?;
			}
			let fold = File::open(old).context("Failed to open old diff file")?;
			let fnew = File::create(new).context("Failed to create destination file")?;

			libfoldiff::upgrade::auto_upgrade::<cliutils::Spinner<false>>(fold, fnew)?;
		},
	}

	Ok(())
}
