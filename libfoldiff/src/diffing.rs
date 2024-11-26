use std::collections::BTreeMap;
use std::fs::File;
use std::io::{copy, Seek, Write};
use camino::{Utf8Path, Utf8PathBuf};
use anyhow::{anyhow, bail, Context};
use rmp_serde::Serializer;
use serde::Serialize;
use zstd::Encoder;
use crate::common::{FoldiffCfg, MAGIC_BYTES, VERSION_NUMBER_LATEST};
use crate::manifest::{DiffManifest, DuplicatedFile, NewFile, PatchedFile};
use crate::{hash, zstddiff};
use crate::reporting::{AutoSpin, Reporter, ReporterSized};

/// An in-memory representation of a diff, used for the diff creation process
#[derive(Clone, Debug, Default)]
pub struct DiffingDiff {
	blobs_new: Vec<Utf8PathBuf>,
	blobs_patch: Vec<Utf8PathBuf>,
	old_root: Utf8PathBuf,
	new_root: Utf8PathBuf,
	files: BTreeMap<u64, DiffingFileData>,
	// for efficient lookups, must be kept in sync
	file_paths_old: BTreeMap<Utf8PathBuf, u64>,
	file_paths_new: BTreeMap<Utf8PathBuf, u64>,
}

/// the looked up value of DiffingDiff::files entries
#[derive(Clone, Debug)]
struct DiffingFileData {
	paths_old: Vec<Utf8PathBuf>,
	paths_new: Vec<Utf8PathBuf>,
	inferred_mime: Option<&'static str>,
}


impl DiffingDiff {
	pub fn new(old_root: Utf8PathBuf, new_root: Utf8PathBuf) -> Self {
		Self {
			old_root,
			new_root,
			..Default::default()
		}
	}

	/// handles finalising an in-memory diffing state to disk
	/// takes mut as it also has to set blobs_new and blobs_patch
	pub fn write_to<TBar: ReporterSized, TSpin: Reporter+Sync>(&mut self, writer: &mut (impl Write + Seek), cfg: &FoldiffCfg) -> anyhow::Result<()> {
		writer.write_all(&MAGIC_BYTES)?;

		// write version number, includes null byte
		writer.write_all(&VERSION_NUMBER_LATEST)?;
		// leave space for length
		writer.write_all(&[0u8; 8])?;

		let mut wr = countio::Counter::new(&mut *writer);
		let mut serializer = Serializer::new(Encoder::new(&mut wr, 19)?.auto_finish());
		self
			.generate_manifest::<TSpin>()?
			.serialize(&mut serializer)
			.context("Failed to serialize diff format into file")?;

		drop(serializer); // load bearing drop
		let comp_size = wr.writer_bytes();
		// write manifest size
		writer.seek_relative(-(comp_size as i64) - 8)?;
		writer.write_all(&comp_size.to_be_bytes())?;
		writer.seek_relative(comp_size as i64)?;

		// write new files
		writer.write_all(&(self.blobs_new.len() as u64).to_be_bytes())?;

		if !self.blobs_new.is_empty() {
			let bar = <TBar as ReporterSized>::new("Compressing new files", self.blobs_new.len());
			for path in &self.blobs_new {
				let mut f =
					File::open(self.new_root.join(path)).context("Failed to open file while copying newly added files")?;

				//writer.write_all(&len.to_be_bytes())?;
				writer.seek_relative(8)?; // space for len

				let mut count = countio::Counter::new(&mut *writer);
				let mut enc = zstd::Encoder::new(&mut count, cfg.level_new as i32)?;
				enc.set_pledged_src_size(Some(f.metadata()?.len()))?;
				enc.include_checksum(false)?;
				enc.include_contentsize(false)?;
				enc.multithread(cfg.threads as u32)?;

				copy(&mut f, &mut enc)?;
				enc.finish()?;

				// write length
				let bytes = count.writer_bytes() as u64;
				writer.seek_relative(-(bytes as i64) - 8)?;
				writer.write_all(&bytes.to_be_bytes())?;
				writer.seek_relative(bytes as i64)?;

				bar.incr(1);
			}
			bar.done();
		}

		// write patches
		writer.write_all(&(self.blobs_patch.len() as u64).to_be_bytes())?;
		//writer.write_all(&0u64.to_be_bytes())?;

		// perform diffing
		if !self.blobs_patch.is_empty() {
			let bar = <TBar as ReporterSized>::new("Diffing changed files", self.blobs_patch.len());
			for p in &self.blobs_patch {
				let mut old = File::open(self.old_root.join(p)).context("Failed to open old file for diffing")?;
				let mut new = File::open(self.new_root.join(p)).context("Failed to open new file for diffing")?;

				let ol = old.metadata()?.len();
				let nl = new.metadata()?.len();

				zstddiff::diff(&mut old, &mut new, &mut *writer, Some(cfg.level_diff), Some(cfg.threads), Some(ol), Some(nl))
					.context("Failed to perform diff")?;
				bar.incr(1);
			}
			bar.done();
		}

		Ok(())
	}

	pub fn write_to_file<TBar: ReporterSized, TSpin: Reporter+Sync>(&mut self, path: &Utf8Path, cfg: &FoldiffCfg) -> anyhow::Result<()> {
		// create file
		let mut f = File::create_new(path).context("Failed to create file to save diff")?;

		self.write_to::<TBar, TSpin>(&mut f, cfg)
	}

	/// generates the on-disk manifest format from the in-memory working data
	/// also populates self.blobs_new and self.blobs_patch
	pub fn generate_manifest<TSpin: Reporter+Sync>(&mut self) -> anyhow::Result<DiffManifest> {
		// generally, the on-disk manifest is a really annoying data structure for building diffs
		// so instead, we work with a map from hash to file data, as if every file was a duplicated one
		// this function will figure out which files fall into which category,
		// and figure out what blobs must be generated by write_to, and generate the manifest.

		// convenience func
		let path_to_string = |p: &Utf8PathBuf| -> anyhow::Result<String> {
			Ok(if cfg!(windows) {
				// path replacement
				assert!(p.is_relative(), "Cannot fix separators in a non-relative path, as this is not accepted by the windows apis for verbatim paths. This should never occur as the diff manifest only contains relative paths.");
				p.as_str().replace('\\', "/")
			} else {
				p.to_string()
			})
		};

		let mut manifest = DiffManifest::default();

		// this is *so* fast that i'm not even going to bother with a progress bar, a spinner is fine.
		let spn = TSpin::new("Sorting scanned files");
		let spn = AutoSpin::spin(&spn);

		for (hash, entry) in &self.files {
			// step 1: are we unchanged?
			if entry.paths_old.len() == 1 && entry.paths_new.len() == 1 && entry.paths_new[0] == entry.paths_old[0] {
				manifest.untouched_files.push((*hash, path_to_string(&entry.paths_old[0])?));
				continue;
			}

			// step 2: are we a duplicate?
			// also handles renames
			if (entry.paths_new.len() == 1 && entry.paths_old.len() == 1) || entry.paths_new.len() > 1 || entry.paths_old.len() > 1 {
				let mut old_paths_utf = Vec::new();
				let mut new_paths_utf = Vec::new();
				old_paths_utf.reserve_exact(entry.paths_old.len());
				new_paths_utf.reserve_exact(entry.paths_new.len());

				for p in &entry.paths_old {
					old_paths_utf.push(path_to_string(p)?);
				}

				for p in &entry.paths_new {
					new_paths_utf.push(path_to_string(p)?);
				}

				// are we *also* a new file?
				let idx =
					if entry.paths_old.is_empty() {
						let i = self.blobs_patch.len() as u64;
						self.blobs_patch.push(entry.paths_new[0].clone());
						i
					}
					else {
						u64::MAX
					};

				manifest.duplicated_files.push(DuplicatedFile {
					old_paths: old_paths_utf,
					new_paths: new_paths_utf,
					idx,
					hash: *hash
				});
				continue;
			}

			// step 3: do we appear new?
			if entry.paths_old.is_empty() {
				debug_assert_eq!(entry.paths_new.len(), 1);
				// do we need to diff?
				let path = &entry.paths_new[0];
				if let Some(old_hash) = self.file_paths_old.get(path) {
					manifest.patched_files.push(PatchedFile {
						old_hash: *old_hash,
						new_hash: *hash,
						path: path_to_string(path)?,
						index: self.blobs_patch.len() as u64
					});
					self.blobs_patch.push(path.clone());
				}
				else {
					// okay, we *are* a new file
					manifest.new_files.push(NewFile {
						hash: *hash,
						path: path_to_string(path)?,
						index: self.blobs_new.len() as u64
					});
					self.blobs_new.push(path.clone());
				}
				continue;
			}

			// step 4: do we appear deleted?
			if entry.paths_new.is_empty() {
				debug_assert_eq!(entry.paths_old.len(), 1);
				// do we need to diff?
				let path = &entry.paths_old[0];

				// if path existed in file_paths_new, we'd generate a diff, but then we'd get doubles
				// as that would be caught in step 3 too, so instead we just ignore in that case
				if !self.file_paths_new.contains_key(path) {
					// okay, we *are* a deleted file
					manifest.deleted_files.push((*hash, path_to_string(path)?));
				}

				continue;
			}

			bail!("All potential scan entry cases should have been handled, but this entry is slipping through the cracks:\n{entry:?}");
		}

		spn.all_good();
		
		// we're done!
		Ok(manifest)
	}

	/// adds a new file to the diff
	/// you should not pass a file that is already in the diff - this will return an Err
	fn add_file(&mut self, in_new: bool, path: &Utf8Path) -> anyhow::Result<()> {
		// check if the path is already there
		let paths = if in_new { &mut self.file_paths_new } else { &mut self.file_paths_old };
		if paths.contains_key(path) {
			bail!("Attempting to add a file to the diff that already exists")
		}

		let root = if in_new { &self.new_root } else { &self.old_root };

		// first, hash it
		let resolved_path = root.join(path);
		let hash = hash::hash_file(&resolved_path)?;

		// get working state
		if let Some(state) = self.files.get_mut(&hash) {
			// add our path
			let state_paths = if in_new { &mut state.paths_new } else { &mut state.paths_old };
			state_paths.push(path.to_path_buf());
			paths.insert(path.to_path_buf(), hash);
		}
		else {
			// perform file type inference
			let inferred_type = infer::get_from_path(&resolved_path).context("Failed to infer file type")?.map(|t| t.mime_type());

			let new_state = DiffingFileData {
				inferred_mime: inferred_type,
				paths_old: if !in_new { vec![path.to_path_buf()] } else { vec![] },
				paths_new: if in_new { vec![path.to_path_buf()] } else { vec![] }
			};

			paths.insert(path.to_path_buf(), hash);

			self.files.insert(hash, new_state);
		}

		Ok(())
	}

	fn scan_internal(&mut self, dir: &Utf8Path, new: bool, spn: &impl Reporter) -> anyhow::Result<()> {
		let root = if new { &self.new_root } else { &self.old_root };
		// we need to clone this, aw
		let root = root.clone();

		// read all files in the root
		let entries = std::fs::read_dir(root.join(dir)).with_context(|| format!("Failed to read dir while scanning {dir:?}"))?;

		for entry in entries {
			let entry = entry.with_context(|| format!("Failed to read entry while scanning {dir:?}"))?;

			spn.incr(1);
			
			// are we a directory or a file?
			let ftype = entry.file_type().context("While reading entry type")?;
			if ftype.is_symlink() {
				bail!("Entry at '{:?}' is a symlink, bailing", entry.path());
			}
			// strip the root off the front of the path else we get errors
			let path: Utf8PathBuf = match entry.path().try_into()
			{
				Ok(p) => p,
				Err(_) => continue, // just ignore non-UTF-8 paths!
			};
			let path = path.strip_prefix(&root)?;
			if ftype.is_dir() {
				// recurse
				self.scan_internal(&path, new, spn)?;
			}
			else {
				// file found!
				self.add_file(new, path).context("While adding file to diff")?;
			}
		}

		Ok(())
	}
}

pub fn scan_to_diff<TSpin: Reporter+Sync>(old_root: Utf8PathBuf, new_root: Utf8PathBuf) -> anyhow::Result<DiffingDiff> {
	let mut new_self = DiffingDiff::new(old_root, new_root);

	let spn = TSpin::new("Scanning old files");
	let aspn = AutoSpin::spin(&spn);
	new_self.scan_internal(Utf8Path::new(""), false, &spn)?;
	aspn.all_good();

	let spn = TSpin::new("Scanning new files");
	let aspn = AutoSpin::spin(&spn);
	new_self.scan_internal(Utf8Path::new(""), true, &spn)?;
	aspn.all_good();

	Ok(new_self)
}