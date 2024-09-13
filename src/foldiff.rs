use crate::{cliutils, zstddiff};
use crate::{handle_res_async, hash, throw_err_async};
use anyhow::{anyhow, bail, ensure, Context, Result};
use derivative::Derivative;
use indicatif::{MultiProgress, ProgressBar};
use memmap2::Mmap;
use rayon::prelude::*;
use rmp_serde::{Deserializer, Serializer};
use serde::{de::IgnoredAny, Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{copy, Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

static VERSION_NUMBER: [u8; 4] = [0x24, 0x09, 0x06, 0x01]; // 2024-09-06 r1

/// internal configuration struct passed into foldiff to control its operation from the cli
#[derive(Copy, Clone, Debug)]
pub struct FldfCfg {
	pub threads: u32,
	pub level_new: u8,
	pub level_diff: u8,
}

/// Messagepack manifest structure stored in the diff file
#[derive(Clone, Debug, Serialize, Deserialize, Derivative)]
#[derivative(Default)]
pub struct DiffManifest {
	#[derivative(Default(value="VERSION_NUMBER"))] // this really should be in std
	version: [u8; 4],
	untouched_files: Vec<HashAndPath>,
	deleted_files: Vec<HashAndPath>,
	new_files: Vec<NewFile>,
	duplicated_files: Vec<DuplicatedFile>,
	patched_files: Vec<PatchedFile>,
}

/// An in-memory representation of a diff, used for the diff creation process
#[derive(Clone, Debug, Default)]
pub struct DiffingDiff {
	// manifest: DiffManifest,
	blobs_new: Vec<PathBuf>,
	blobs_patch: Vec<PathBuf>,
	old_root: PathBuf,
	new_root: PathBuf,
	files: BTreeMap<u64, DiffingFileData>,
	// for efficient lookups, must be kept in sync
	file_paths_old: BTreeMap<PathBuf, u64>,
	file_paths_new: BTreeMap<PathBuf, u64>,
}

/// the looked up value of DiffingDiff::files entries
#[derive(Clone, Debug)]
struct DiffingFileData {
	paths_old: Vec<PathBuf>,
	paths_new: Vec<PathBuf>,
	inferred_mime: Option<&'static str>,
}

/// An in-memory representation of a diff, used for the applying process
#[derive(Debug, Default)]
pub struct ApplyingDiff {
	manifest: DiffManifest,
	blobs_new: Vec<u64>,   // offset into diff file
	blobs_patch: Vec<u64>, // offset into diff file
	read: Option<Mmap>, // the diff file map
	old_root: PathBuf,
	new_root: PathBuf,
}

// untouched files, deleted files
type HashAndPath = (u64, String);

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct NewFile {
	hash: u64,
	index: u64,
	path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct DuplicatedFile {
	hash: u64,
	old_paths: Vec<String>,
	new_paths: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct PatchedFile {
	old_hash: u64,
	new_hash: u64,
	index: u64,
	path: String,
}

/*impl DiffManifest {
	/// checks if this diff state contains a reference to the given path in the old folder
	/// this does not check if the hash matches, but does return it if present
	fn contains_file(&self, root_is_new: bool, path: &Path) -> Option<u64> {
		if !root_is_new {
			for (h, p) in &self.deleted_files {
				if Path::new(p) == path {
					return Some(*h);
				}
			}
		}

		for (h, p) in &self.untouched_files {
			if Path::new(p) == path {
				return Some(*h);
			}
		}

		if root_is_new {
			for nf in &self.new_files {
				if Path::new(&nf.path) == path {
					return Some(nf.hash);
				}
			}
		}

		for dup in &self.duplicated_files {
			let paths = if root_is_new { &dup.new_paths } else { &dup.old_paths };

			if paths.iter().any(|p| Path::new(p) == path) {
				return Some(dup.hash);
			}
		}

		for pat in &self.patched_files {
			if Path::new(&pat.path) == path {
				return Some(if root_is_new { pat.new_hash } else { pat.old_hash });
			}
		}

		None
	}

	/// checks if the given path is present in the diff and verifies that it matches the expected hash
	/// if this returns false, the directory structure on disk does not match that dictated by the state
	fn verify_contains(&self, root_is_new: bool, path: &Path, root: &Path) -> Result<Option<bool>> {
		if let Some(hash) = self.contains_file(root_is_new, path) {
			let hash_actual = hash::hash_file(&root.join(path))?;

			Ok(Some(hash == hash_actual))
		}
		else {
			Ok(None)
		}
	}
}*/

impl DiffingDiff {
	pub fn new(old_root: PathBuf, new_root: PathBuf) -> Self {
		Self {
			old_root,
			new_root,
			..Default::default()
		}
	}

	/// handles finalising an in-memory diffing state to disk
	/// takes mut as it also has to set blobs_new and blobs_patch
	pub fn write_to(&mut self, writer: &mut (impl Write + Seek), cfg: &FldfCfg) -> Result<()> {
		writer.write_all("FLDF".as_bytes())?;

		let mut serializer = Serializer::new(&mut *writer); // lol re-borrowing is goofy but sure
		self
			.generate_manifest()?
			.serialize(&mut serializer)
			.context("Failed to serialize diff format into file")?;
		drop(serializer); // this drops here anyway, but is load-bearing, so make it explicit

		// write new files
		writer.write_all(&(self.blobs_new.len() as u64).to_be_bytes())?;

		if !self.blobs_new.is_empty() {
			let bar = cliutils::create_bar("Compressing new files", self.blobs_new.len() as u64, true);
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
				enc.multithread(cfg.threads)?;

				_ = copy(&mut f, &mut enc)?;
				enc.finish()?;

				// write length
				let bytes = count.writer_bytes() as u64;
				writer.seek_relative(-(bytes as i64) - 8)?;
				writer.write_all(&bytes.to_be_bytes())?;
				writer.seek_relative(bytes as i64)?;

				bar.inc(1);
			}
			cliutils::finish_bar(&bar);
		}

		// write patches
		writer.write_all(&(self.blobs_patch.len() as u64).to_be_bytes())?;
		//writer.write_all(&0u64.to_be_bytes())?;

		// perform diffing
		if !self.blobs_patch.is_empty() {
			let bar = cliutils::create_bar("Diffing changed files", self.blobs_patch.len() as u64, true);
			for p in &self.blobs_patch {
				let mut old = File::open(self.old_root.join(p)).context("Failed to open old file for diffing")?;
				let mut new = File::open(self.new_root.join(p)).context("Failed to open new file for diffing")?;

				let ol = old.metadata()?.len();
				let nl = new.metadata()?.len();

				zstddiff::diff(&mut old, &mut new, &mut *writer, Some(cfg.level_diff), Some(cfg.threads), Some(ol), Some(nl))
					.context("Failed to perform diff")?;
				bar.inc(1);
			}
			cliutils::finish_bar(&bar);
		}

		Ok(())
	}

	pub fn write_to_file(&mut self, path: &Path, cfg: &FldfCfg) -> Result<()> {
		// create file
		let mut f = File::create_new(path).context("Failed to create file to save diff")?;

		self.write_to(&mut f, cfg)
	}

	/// generates the on-disk manifest format from the in-memory working data
	/// also populates self.blobs_new and self.blobs_patch
	pub fn generate_manifest(&mut self) -> Result<DiffManifest> {
		// generally, the on-disk manifest is a really annoying data structure for building diffs
		// so instead, we work with a map from hash to file data, as if every file was a duplicated one
		// this function will figure out which files fall into which category,
		// and figure out what blobs must be generated by write_to, and generate the manifest.

		// convenience func
		let path_to_string = |p: &PathBuf| -> Result<String> {
			Ok(p.to_str().ok_or(anyhow!("Found a non-UTF-8 path name. Just no. Why."))?.to_string())
		};

		let mut manifest = DiffManifest::default();

		// this is *so* fast that i'm not even going to bother with a progress bar.
		//let bar = cliutils::create_bar("Sorting scanned files", self.files.len() as u64);
		let spn = cliutils::create_spinner("Sorting scanned files", false, true);
		spn.enable_steady_tick(Duration::from_millis(150));

		// mime types of stored patches and of new files
		let mut patched_with_types = Vec::new();
		let mut new_with_types = Vec::new();

		for (hash, entry) in &self.files {
			// tick the bar
			//bar.inc(1);

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

				manifest.duplicated_files.push(DuplicatedFile {
					old_paths: old_paths_utf,
					new_paths: new_paths_utf,
					hash: *hash
				});
				continue;
			}

			// step 3: do we appear new?
			if entry.paths_old.is_empty() {
				debug_assert_eq!(entry.paths_new.len(), 1);
				// do we need to diff?
				let path = &entry.paths_new[0];
				if let Some(new_hash) = self.file_paths_old.get(path) {
					manifest.patched_files.push(PatchedFile {
						old_hash: *hash,
						new_hash: *new_hash,
						path: path_to_string(path)?,
						index: patched_with_types.len() as u64
					});
					patched_with_types.push((path.clone(), entry.inferred_mime));
				}
				else {
					// okay, we *are* a new file
					manifest.new_files.push(NewFile {
						hash: *hash,
						path: path_to_string(path)?,
						index: new_with_types.len() as u64
					});
					new_with_types.push((path.clone(), entry.inferred_mime));
				}
				continue;
			}

			// step 4: do we appear deleted?
			if entry.paths_new.is_empty() {
				debug_assert_eq!(entry.paths_old.len(), 1);
				// do we need to diff?
				let path = &entry.paths_old[0];
				if let Some(old_hash) = self.file_paths_new.get(path) {
					manifest.patched_files.push(PatchedFile {
						old_hash: *old_hash,
						new_hash: *hash,
						path: path_to_string(path)?,
						index: patched_with_types.len() as u64
					});
					patched_with_types.push((path.clone(), entry.inferred_mime));
				}
				else
				{
					// okay, we *are* a deleted file
					manifest.deleted_files.push((*hash, path_to_string(path)?));
				}

				continue;
			}

			bail!("All potential scan entry cases should have been handled, but this entry is slipping through the cracks:\n{entry:?}");
		}

		// now, sort the list of diffs by file type
		// :sparkles: logic deduplication :sparkles:
		for l in [&mut patched_with_types, &mut new_with_types] {
			// splits a path by `/`, and reverses the order of the splits
			let swap_name = |p: &PathBuf| {
				p.to_str().map(|s| {
					s.rsplit("/").map(String::from).collect::<Box<[_]>>()
				})
			};

			// in rust you can use a tuple's ordering impl to do sort-by-then-by
			// it will use the first element unless they return Ordering::Equal, then onto onto the next, etc
			l.sort_by_key(|p| (p.1, swap_name(&p.0)));
		}

		// put the sorted arrays back into place
		self.blobs_patch.extend(patched_with_types.into_iter().map(|p| p.0));
		self.blobs_new.extend(new_with_types.into_iter().map(|p| p.0));

		cliutils::finish_spinner(&spn, false);

		// we're done!
		Ok(manifest)
	}

	/// adds a new file to the diff
	/// you should not pass a file that is already in the diff - this will return an Err
	fn add_file(&mut self, in_new: bool, path: &Path) -> Result<()> {
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

	pub fn scan(old_root: PathBuf, new_root: PathBuf) -> Result<Self> {
		let mut new_self = Self::new(old_root, new_root);

		let bar = cliutils::create_spinner("Scanning old files", true, true);
		new_self.scan_internal(Path::new(""), false, Some(&bar))?;
		cliutils::finish_spinner(&bar, true);

		let bar = cliutils::create_spinner("Scanning new files", true, true);
		new_self.scan_internal(Path::new(""), true, Some(&bar))?;
		cliutils::finish_spinner(&bar, true);

		Ok(new_self)
	}

	fn scan_internal(&mut self, dir: &Path, new: bool, spinner: Option<&ProgressBar>) -> Result<()> {
		let root = if new { &self.new_root } else { &self.old_root };
		// we need to clone this, aw
		let root = root.clone();

		// read all files in the root
		let entries = std::fs::read_dir(root.join(dir)).with_context(|| format!("Failed to read dir while scanning {dir:?}"))?;

		for entry in entries {
			let entry = entry.with_context(|| format!("Failed to read entry while scanning {dir:?}"))?;

			// tick!
			if let Some(s) = spinner {
				s.inc(1);
			}

			// are we a directory or a file?
			let ftype = entry.file_type().context("While reading entry type")?;
			if ftype.is_symlink() {
				bail!("Entry at '{:?}' is a symlink, bailing", entry.path());
			}
			if ftype.is_dir() {
				// recurse
				self.scan_internal(&entry.path(), new, spinner)?;
			}
			else {
				// file found!
				// strip the root off the front of the path
				// else we get errors in add_file
				let path = entry.path();
				let path = path.strip_prefix(&root)?;
				self.add_file(new, path).context("While adding file to diff")?;
			}

			// sleep for progress bar testing
			//std::thread::sleep_ms(600);
		}

		Ok(())
	}
}

impl ApplyingDiff {
	fn read_from(reader: &mut (impl Read + Seek)) -> Result<Self> {
		// check magic bytes
		let mut magic = [0u8, 0, 0, 0];
		reader
			.read_exact(&mut magic)
			.context("Failed to read while creating diff format")?;
		ensure!(
			magic == "FLDF".as_bytes(),
			"Magic bytes did not match expectation ({magic:x?} instead of 'FLDF')"
		);

		// deserialize msgpack data
		// this better understand when to stop reading lol
		let mut deserializer = Deserializer::new(&mut *reader);
		let manifest =
			DiffManifest::deserialize(&mut deserializer).context("Failed to deserialize diff format")?;
		drop(deserializer); // this drops here anyway, but is load-bearing, so make it explicit

		// check version
		ensure!(
			manifest.version == VERSION_NUMBER,
			"Did not recognise version number {:x?}",
			manifest.version
		);

		// create self
		let mut new_self = Self::default();
		new_self.manifest = manifest;

		let mut new_blob_count = [0u8, 0, 0, 0, 0, 0, 0, 0];
		reader
			.read_exact(&mut new_blob_count)
			.context("Failed to read new file count")?;
		let new_blob_count = u64::from_be_bytes(new_blob_count);

		for _ in 0..new_blob_count {
			// keep track of the offset
			new_self.blobs_new.push(reader.stream_position()?);

			// read blob length
			let mut len = [0u8, 0, 0, 0, 0, 0, 0, 0];
			reader
				.read_exact(&mut len)
				.context("Failed to read new file length")?;
			let len = u64::from_be_bytes(len);

			// keep track of the offset
			new_self.blobs_new.push(reader.stream_position()?);
			// jump to next file
			reader
				.seek_relative(len.try_into()?)
				.context("Failed to seek while skipping new file")?;
		}

		let mut patched_blob_count = [0u8, 0, 0, 0, 0, 0, 0, 0];
		reader
			.read_exact(&mut patched_blob_count)
			.context("Failed to read patched file count")?;
		let patched_blob_count = u64::from_be_bytes(patched_blob_count);

		for _ in 0..patched_blob_count {
			// keep track of the offset
			new_self.blobs_new.push(reader.stream_position()?);

			// read through array
			// this is not that efficient but oh well
			let mut deser = Deserializer::new(&mut *reader);
			// lol name collision
			serde::Deserializer::deserialize_any(&mut deser, IgnoredAny)
				.context("Failed to read through patched file data")?;
		}

		Ok(new_self)
	}

	/// handles initialising an in-memory applying state from disk
	pub fn read_from_file(path: &Path) -> Result<Self> {
		let f = File::open(path).context("Failed to open file to read diff")?;

		// safety: UB if the underlying diff is modified by someone else
		// todo: is this just acceptable? do we need to lock the file (unix only) or equivalent?
		let map = unsafe { Mmap::map(&f) }?;

		let mut res = Self::read_from(&mut Cursor::new(&map))?;
		res.read = Some(map);
		Ok(res)
	}

	pub fn apply(&mut self, old_root: PathBuf, new_root: PathBuf, cfg: &FldfCfg) -> Result<()> {
		self.old_root = old_root;
		self.new_root = new_root;

		let diff_map = &**self.read.as_ref().ok_or(anyhow!("Cannot call apply() on a state without a set `read` prop"))?;

		let num_duped_files: u64 = self.manifest.duplicated_files.iter().map(|d| d.new_paths.len() as u64).sum();

		// incr bar and finish if done
		let inc_n = |n: u64, b: &ProgressBar| {
			b.inc(n);
			if Some(b.position()) == b.length() {
				cliutils::finish_bar(b);
			}
		};
		let inc = |b: &ProgressBar| inc_n(1, b);

		// progress reporting
		let wrap = MultiProgress::new();
		let spn = wrap.add(cliutils::create_spinner("Applying diff", false, false));
		let bar_untouched = wrap.add(cliutils::create_bar("Copying unchanged files", (self.manifest.untouched_files.len() as u64)  + num_duped_files, false));
		let bar_new = wrap.add(cliutils::create_bar("Creating new files", self.manifest.new_files.len() as u64, false));
		let bar_patched = wrap.add(cliutils::create_bar("Applying patched files", self.manifest.patched_files.len() as u64, false));

		// need to do this manually because of it being in a wrap
		for b in [&spn, &bar_untouched, &bar_new, &bar_patched] {
			b.enable_steady_tick(Duration::from_millis(50));
		}
		
		// we need to create the directory to apply into
		std::fs::create_dir_all(&self.new_root)?;

		// let's spawn some threads!
		let errs = Mutex::new(Vec::new());
		rayon::ThreadPoolBuilder::new()
			.num_threads(cfg.threads as usize)
			.use_current_thread()
			.build()?
			.scope(|s| {
				if self.manifest.untouched_files.is_empty() && self.manifest.duplicated_files.is_empty() {
					bar_untouched.finish_and_clear();
				}
				else {
					s.spawn(|_| {
						// handle untouched files
						for (h, p) in &self.manifest.untouched_files {
							// std:;fs::copy would be faster, but we want to verify the hash
							let mut src = handle_res_async!(errs, File::open(self.old_root.join(p)), "Failed to open file to copy from {}", p);
							let mut dst = handle_res_async!(errs, File::create(self.new_root.join(p)), "Failed to create file to copy to {}", p);

							let mut hw = hash::HashStreamer::new(&mut dst);
							handle_res_async!(errs, std::io::copy(&mut src, &mut hw), "Failed to copy file {}", p);

							let rh = hw.finish();
							if rh != *h {
								throw_err_async!(errs, anyhow!("Found {p} was different to expected (hash was {rh}, not {})", *h));
							}

							inc(&bar_untouched);
						}
					});
					s.spawn(|_| {
						// handle duplicated files
						for d in &self.manifest.duplicated_files {
							// check all the hashes match
							let mut checks: Vec<_> =
								d.old_paths
									.par_iter()
									.filter_map(|p| {
										let mut f = match File::open(self.old_root.join(p)).context(format!("Failed to open old file {p} to verify hash")) {
											Ok(f) => f,
											Err(e) => return Some(e),
										};
										let h = match hash::hash_stream(&mut f).context(format!("Failed to hash old file {p} to verify it")) {
											Ok(f) => f,
											Err(e) => return Some(e),
										}; // shame `?` doesn't work well
	
										if h != d.hash {
											Some(anyhow!("Old file {p} was not as expected."));
										}
										None
									})
									.collect();
							
							if !checks.is_empty() {
								errs.lock().unwrap().extend(checks.drain(..));
								return;
							}
							
							// okay, now copy to all the new places then
							let mut checks: Vec<_> = d.new_paths
								.par_iter()
								.filter_map(|p| {
									// performs an in-kernel copy for speed
									match std::fs::copy(self.old_root.join(&d.old_paths[0]), self.new_root.join(p)).context(format!("Failed to copy file {p}")) {
										Ok(_bytes_copied) => {},
										Err(e) => return Some(e),
									};
									None
								})
								.collect();

							if !checks.is_empty() {
								errs.lock().unwrap().extend(checks.drain(..));
								return;
							}
							
							inc_n(d.new_paths.len() as u64, &bar_untouched);
						}
					});
				}
				if self.manifest.new_files.is_empty() {
					bar_new.finish_and_clear();
				}
				else {
					s.spawn(|_| {
						// handle new files
						for nf in &self.manifest.new_files {
							let blob = if let Some(t) = self.blobs_new.get(nf.index as usize) {
									*t as usize
								}
								else {
									throw_err_async!(errs, anyhow!("new file {} had an out-of-range index pointing to its data", nf.path));
								};

							// create new file
							let mut dest = handle_res_async!(errs, File::create(self.new_root.join(&nf.path)), "Failed to create {} to write new file", &nf.path);
							let mut wrt = hash::HashStreamer::new(&mut dest);

							// copy and decompress
							let mut read = Cursor::new(&diff_map[blob..]);

							handle_res_async!(errs, zstd::stream::copy_decode(&mut read, &mut wrt), "Failed to decompress file {}", &nf.path);

							let rh = wrt.finish();
							if rh != nf.hash {
								throw_err_async!(errs, anyhow!("Written {} was different to expected (hash was {rh}, not {})", nf.path, nf.hash));
							}

							inc(&bar_new);
						}
					});
				}
				if self.manifest.patched_files.is_empty() {
					bar_patched.finish_and_clear();
				}
				else {
					s.spawn(|_| {
						// handle patched files
						for pf in &self.manifest.patched_files {
							let mut src = handle_res_async!(errs, File::open(self.old_root.join(&pf.path)), "Failed to open file to patch from {}", pf.path);
							let mut dst = handle_res_async!(errs, File::create(self.new_root.join(&pf.path)), "Failed to create file to patch to {}", pf.path);

							// get length of src
							let src_len = handle_res_async!(errs, src.metadata(), "Couldn't get length of patch source file {}", pf.path).len();

							let mut src = hash::HashStreamer::new(&mut src);
							let mut dst = hash::HashStreamer::new(&mut dst);

							let blob = if let Some(t) = self.blobs_patch.get(pf.index as usize) {
								*t as usize
							}
							else {
								throw_err_async!(errs, anyhow!("patched file {} had an out-of-range index pointing to its data", pf.path));
							};

							// get diff blob ready
							let mut diff = Cursor::new(&diff_map[blob..]);

							// apply!
							handle_res_async!(errs,
								zstddiff::apply(&mut src, &mut diff, &mut dst, Some(src_len)),
								"Failed to apply diff for {}", pf.path);

							let src_rh = src.finish();
							let dst_rh = dst.finish();
							if src_rh != pf.old_hash {
								throw_err_async!(errs, anyhow!("Source {} was different to expected (hash was {src_rh}, not {})", pf.path, pf.old_hash));
							}
							if dst_rh != pf.new_hash {
								throw_err_async!(errs, anyhow!("Written {} was different to expected (hash was {dst_rh}, not {})", pf.path, pf.new_hash));
							}

							inc(&bar_patched);
						}
					});
				}
			});

		let mut errs = errs.lock().unwrap();
		if !errs.is_empty() {
			if errs.len() == 1 {
				return Err(errs.pop().unwrap());
			}
			bail!("Failed with multiple errors:\n{}", errs.iter().map(|e| format!("{e:?}")).reduce(|a, b| a + "\n" + &*b).unwrap())
		}

		cliutils::finish_spinner(&spn, false);
		Ok(())
	}
}