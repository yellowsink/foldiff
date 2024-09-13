use std::collections::BTreeSet;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use anyhow::{bail, Result};
use indicatif::ProgressBar;
use crate::cliutils;
use crate::hash::hash_file;

pub enum Mismatch {
	TypeMismatch((PathBuf, PathBuf)), // (File, Folder)
	HashMismatch(PathBuf),
	OnlyIn((PathBuf, bool)), // bool is "is in second dir"
}

impl Display for Mismatch {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Mismatch::TypeMismatch((p1, p2)) => 
				write!(f, "{p1:?} is a file, but {p2:?} is a folder, thus they mismatch."),
			Mismatch::HashMismatch(p) =>
				write!(f, "The file {p:?} exists in both directories, but has differing contents."),
			Mismatch::OnlyIn((p, in_second)) =>
				if *in_second {
					write!(f, "{p:?} only exists in the second folder.")
				}
				else {
					write!(f, "{p:?} only exists in the first folder.")
				},
		}
	}
}

/// Checks if two directories are identical
pub fn test_equality(r1: &Path, r2: &Path) -> Result<Vec<Mismatch>> {
	let mut mismatches = Vec::new();
	let spinner = cliutils::create_spinner("Scanning folders", true, true);
	test_equality_internal(r1, r2, Path::new(""), &spinner, &mut mismatches)?;
	cliutils::finish_spinner(&spinner, true);
	Ok(mismatches)
}

fn test_equality_internal(r1: &Path, r2: &Path, p: &Path, spn: &ProgressBar, mismatches: &mut Vec<Mismatch>) -> Result<()> {
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

	spn.inc(1);
	
	if type1.is_file() {
		if type2.is_file() {
			if hash_file(&path1)? != hash_file(&path2)? {
				mismatches.push(Mismatch::HashMismatch(p.to_path_buf()));
			}
		}
		else {
			mismatches.push(Mismatch::TypeMismatch((
				Path::new(r1.file_name().unwrap()).join(p),
				Path::new(r2.file_name().unwrap()).join(p)
			)));
		}
	}
	else if type2.is_file() {
		mismatches.push(Mismatch::TypeMismatch((
			Path::new(r2.file_name().unwrap()).join(p),
			Path::new(r1.file_name().unwrap()).join(p)
		)));
	}
	else {
		// both are directories
		
		let files1: std::io::Result<Vec<_>> = fs::read_dir(path1)?.collect();
		let files2: std::io::Result<Vec<_>> = fs::read_dir(path2)?.collect();

		let set1 = BTreeSet::from_iter(files1?.iter().map(|e| e.file_name()));
		let set2 = BTreeSet::from_iter(files2?.iter().map(|e| e.file_name()));
		
		// first, check for files only in set 1
		for f in &set1 {
			if !set2.contains(f) {
				mismatches.push(Mismatch::OnlyIn((p.join(f), false)));
				spn.inc(1);
			}
			else {
				// we have both! recurse.
				test_equality_internal(r1, r2, &p.join(f), spn, mismatches)?
			}
		}
		// finally, check for files only in set 2
		for f in set2 {
			if !set1.contains(&f) {
				mismatches.push(Mismatch::OnlyIn((p.join(f), false)));
				spn.inc(1);
			}
		}
	}
	
	Ok(())
}
