use crate::manifest::DiffManifest;
use crate::hash::hash_file;
use crate::aggregate_errors;
use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use std::collections::BTreeSet;
use std::fs;
use camino::{Utf8Path, Utf8PathBuf};
use crate::reporting::{AutoSpin, Reporter};

/// Checks if two directories are identical, printing results to stdout
pub fn test_dir_equality<TSpin: Reporter+Sync>(r1: &Utf8Path, r2: &Utf8Path) -> Result<()> {
	let spn = TSpin::new("Scanning folders");
	let aspn = AutoSpin::spin(&spn);
	test_equality_internal(r1, r2, "".into(), &spn)?;
	aspn.all_good();
	Ok(())
}

fn test_equality_internal(r1: &Utf8Path, r2: &Utf8Path, p: &Utf8Path, spn: &(impl Reporter+Sync)) -> Result<()> {
	// stat both paths
	let path1 = r1.join(p);
	let path2 = r2.join(p);
	let type1 = fs::symlink_metadata(&path1)?;
	let type2 = fs::symlink_metadata(&path2)?;

	if type1.is_symlink() {
		bail!("Found a symlink at {:?}", path1);
	}
	if type2.is_symlink() {
		bail!("Found a symlink at {:?}", path2);
	}

	spn.incr(1);

	if type1.is_file() {
		if type2.is_file() {
			if hash_file(&path1)? != hash_file(&path2)? {
				spn.suspend(|| {
					println!("The file {:?} exists in both directories, but has differing contents.", p.to_path_buf());
				});
			}
		}
		else {
			spn.suspend(|| {
				println!(
					"{:?} is a file, but {:?} is a folder, thus they mismatch.",
					Utf8Path::new(r1.file_name().unwrap()).join(p),
					Utf8Path::new(r2.file_name().unwrap()).join(p)
				);
			});
		}
	}
	else if type2.is_file() {
		spn.suspend(|| {
			println!(
				"{:?} is a folder, but {:?} is a file, thus they mismatch.",
				Utf8Path::new(r1.file_name().unwrap()).join(p),
				Utf8Path::new(r2.file_name().unwrap()).join(p)
			);
		});
	}
	else {
		// both are directories

		let files1: std::io::Result<Vec<_>> = fs::read_dir(path1)?.collect();
		let files2: std::io::Result<Vec<_>> = fs::read_dir(path2)?.collect();

		let set1 = BTreeSet::<Utf8PathBuf>::from_iter(files1?.iter().filter_map(|e| e.file_name().to_str().map(Into::into)));
		let set2 = BTreeSet::<Utf8PathBuf>::from_iter(files2?.iter().filter_map(|e| e.file_name().to_str().map(Into::into)));

		let mut rec_res = anyhow::Ok(());
		// do the loops in parallel
		rayon::join(
			// check for files only in set 1, and for files in both
			|| {
				rec_res =
					set1.par_iter()
						.map(|f| {
							if !set2.contains(f) {
								spn.suspend(|| {
									println!("{:?} only exists in the first folder.", p.join(f));
								});
								spn.incr(1);
							}
							else {
								// we have both! recurse.
								test_equality_internal(r1, r2, &p.join(f), spn)?
							}
							Ok(())
						})
						.collect();
			},
			||
				set2.par_iter()
					.for_each(|f| {
						if !set1.contains(f) {
							spn.suspend(|| {
								println!("{:?} only exists in the second folder.", p.join(f));
							});
							spn.incr(1);
						}
					}),
		);

		rec_res?;
	}

	Ok(())
}

/// Checks if two directories match the given manifest, printing results to stdout
pub fn verify_against_diff<TSpin: Reporter+Sync>(r1: &Utf8Path, r2: &Utf8Path, manifest: &DiffManifest) -> Result<()> {
	let spn = TSpin::new("Verifying files");
	let aspn = AutoSpin::spin(&spn);

	let errors: Vec<_> =
		manifest.untouched_files
			.par_iter()
			.flat_map(|(h, p)| [(*h, r1.join(p)), (*h, r2.join(p))])
			.chain(
				manifest.deleted_files.par_iter()
					.map(|(h, p)| (*h, r1.join(&p)))
			)
			.chain(
				manifest.new_files.par_iter()
					.map(|nf| (nf.hash, r2.join(&nf.path)))
			)
			.chain(
				manifest.patched_files.par_iter()
					.flat_map(|pf| [(pf.old_hash, r1.join(&pf.path)), (pf.new_hash, r2.join(&pf.path))])
			)
			.chain(
				manifest.duplicated_files.par_iter()
					.flat_map(|df| {
						df.old_paths.iter().map(|p| r1.join(p))
							.chain(df.new_paths.iter().map(|p| r2.join(p)))
							.map(|p| (df.hash, p))
							.collect::<Vec<_>>() // make par_iter happy
					})
			)
			.map(|(h, p)| {
				if !fs::exists(&p).context(format!("Failed to check if {p:?} exists"))? {
					spn.suspend(|| {
						println!("{p:?} is missing");
					})
				}
				else if hash_file(&p).context(format!("Failed to hash file {p:?}"))? != h {
					spn.suspend(|| {
						println!("{p:?} is not as expected");
					})
				}
				spn.incr(1);
				anyhow::Ok(())
			})
			.filter_map(|r| match r {
				Ok(()) => None,
				Err(e) => Some(e),
			})
			.collect();

	aspn.all_good();

	aggregate_errors!(errors);

	Ok(())
}