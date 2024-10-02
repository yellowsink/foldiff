use crate::common::{copy_rl, copy_rl_hash, create_file};
use crate::manifest::DiffManifest;
use crate::reporting::{AutoSpin, CanBeWrappedBy, Reporter, ReporterSized, ReportingMultiWrapper};
use crate::{aggregate_errors, handle_res_async, handle_res_parit, hash, throw_err_async, zstddiff};
use anyhow::{anyhow, Context};
use memmap2::Mmap;
use rayon::prelude::*;
use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

impl ApplyingDiff {
	pub fn apply<
		TWrap: ReportingMultiWrapper,
		TSpin: Reporter + CanBeWrappedBy<TWrap> + Sync,
		TBar: ReporterSized + CanBeWrappedBy<TWrap> + Sync
	>(&mut self, old_root: PathBuf, new_root: PathBuf) -> anyhow::Result<()> {
		self.old_root = old_root;
		self.new_root = new_root;

		let diff_map = &**self.read.as_ref().ok_or(anyhow!("Cannot call apply() on a state without a set `read` prop"))?;

		let num_duped_copy: usize = self.manifest.duplicated_files.iter().filter(|d| d.idx == u64::MAX).map(|d| d.new_paths.len()).sum();
		let num_duped_create: usize = self.manifest.duplicated_files.iter().filter(|d| d.idx != u64::MAX).map(|d| d.new_paths.len()).sum();

		// incr bar and finish if done
		let inc_n = |n: usize, b: &TBar| {
			b.incr(n);
			if b.count() == b.length() {
				b.done();
			}
		};
		let inc = |b: &TBar| inc_n(1, b);

		// progress reporting
		let wrap = TWrap::new();
		let spn = TSpin::new("Applying diff").add_to(&wrap);
		let bar_untouched = <TBar as ReporterSized>::new("Copying unchanged files", self.manifest.untouched_files.len() + num_duped_copy).add_to(&wrap);
		let bar_new = <TBar as ReporterSized>::new("Creating new files", self.manifest.new_files.len() + num_duped_create).add_to(&wrap);
		let bar_patched = <TBar as ReporterSized>::new("Applying patched files", self.manifest.patched_files.len()).add_to(&wrap);

		let as1 = AutoSpin::spin(&spn);
		let as2 = AutoSpin::spin(&bar_untouched);
		let as3 = AutoSpin::spin(&bar_new);
		let as4 = AutoSpin::spin(&bar_patched);

		// let's spawn some threads!
		let errs = Mutex::new(Vec::new());
		rayon::scope(|s| {
			if self.manifest.untouched_files.is_empty() && self.manifest.duplicated_files.is_empty() {
				bar_untouched.done_clear();
			}
			else {
				s.spawn(|_| {
					// handle untouched files
					// use a parallel iterator so we can use as MANY threads as possible,
					// or for if the other tasks are all done first.
					let mut checks: Vec<_> =
						self.manifest.untouched_files
							.par_iter()
							.filter_map(|(h, p)| {
								let h = *h;
								let old_path = self.old_root.join(p);
								let new_path = self.new_root.join(p);
								
								let real_hash = handle_res_parit!(copy_rl_hash(old_path, new_path));
								
								if real_hash != h {
									return Some(anyhow!("Found {p} was different to expected (hash was {real_hash}, not {})", h));
								}

								inc(&bar_untouched);
								None
							})
							.collect();

					if !checks.is_empty() {
						errs.lock().unwrap().extend(checks.drain(..));
					}
				});
				s.spawn(|_| {
					// handle duplicated files
					// could be further parallelized by turning this loop into a par_iter,
					// but seems unnecessary to me due to this already being pretty parallelized.
					for d in &self.manifest.duplicated_files {
						// check all the hashes match
						let mut checks: Vec<_> =
							d.old_paths
								.par_iter()
								.filter_map(|p| {
									let mut f = handle_res_parit!(File::open(self.old_root.join(p)), "Failed to open old file {p} to verify hash");
									let h = handle_res_parit!(hash::hash_stream(&mut f), "Failed to hash old file {p} to verify it");

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
						// if we have a file on disk, then perform an in-kernel copy for speed
						let mut checks: Vec<_> =
							if d.idx == u64::MAX {
								d.new_paths
									.par_iter()
									.filter_map(|p| {
										// ensure we have a parent directory
										let dest_path = self.new_root.join(p);
										if let Some(par) = dest_path.parent() {
											handle_res_parit!(std::fs::create_dir_all(par), "Failed to create parent dir to copy file {p}");
										}

										handle_res_parit!(copy_rl(self.old_root.join(&d.old_paths[0]), dest_path), "Failed to copy file {p}");
										None
									})
									.collect()
							}
							else {
								// we need to copy out of ourself
								let blob = if let Some(t) = self.blobs_new.get(d.idx as usize) {
									*t as usize
								}
								else {
									throw_err_async!(errs, anyhow!("new file {} had an out-of-range index pointing to its data", d.new_paths[0]));
								};

								// read length
								let len = u64::from_be_bytes(*diff_map[blob..].first_chunk().unwrap()) as usize;
								let blob = blob + 8; // advance past length
								
								// copy one out
								let p = &d.new_paths[0];
								let mut read = Cursor::new(&diff_map[blob..(blob + len)]);
								let f = handle_res_async!(errs, create_file(&self.new_root.join(p)), "Failed to create new file {p} to write to");
								let mut writer = hash::XXHashStreamer::new(f);

								handle_res_async!(errs, std::io::copy(&mut read, &mut writer));

								// check hash
								let rh = writer.finish();
								if rh != d.hash {
									throw_err_async!(errs, anyhow!("Newly created file {p} does not match expected data"));
								}
								
								// copy to the rest
								d.new_paths
									.par_iter()
									.skip(1)
									.filter_map(|p| {
										// ensure we have a parent directory
										let dest_path = self.new_root.join(p);
										if let Some(par) = dest_path.parent() {
											handle_res_parit!(std::fs::create_dir_all(par), "Failed to create parent dir to copy file {p}");
										}

										handle_res_parit!(copy_rl(self.old_root.join(&d.old_paths[0]), dest_path), "Failed to copy file {p}");
										None
									})
									.collect()
							};

						if !checks.is_empty() {
							errs.lock().unwrap().extend(checks.drain(..));
							return;
						}

						inc_n(d.new_paths.len(), if d.idx == u64::MAX { &bar_untouched } else { &bar_new });
					}
				});
			}
			if self.manifest.new_files.is_empty() {
				bar_new.done_clear();
			}
			else {
				s.spawn(|_| {
					// handle new files
					let mut checks: Vec<_> = self.manifest.new_files
						.par_iter()
						.filter_map(|nf| {
							let blob = if let Some(t) = self.blobs_new.get(nf.index as usize) {
								*t as usize
							}
							else {
								return Some(anyhow!("new file {} had an out-of-range index pointing to its data", nf.path));
							};

							// create new file
							let mut dest = handle_res_parit!(create_file(&self.new_root.join(&nf.path)), "Failed to create {} to write new file", &nf.path);
							let mut wrt = hash::XXHashStreamer::new(&mut dest);

							// read length
							let len = u64::from_be_bytes(*diff_map[blob..].first_chunk().unwrap()) as usize;
							let blob = blob + 8; // advance past length

							// copy and decompress
							let mut read = Cursor::new(&diff_map[blob..(blob + len)]);

							handle_res_parit!(zstd::stream::copy_decode(&mut read, &mut wrt), "Failed to decompress file {}", &nf.path);

							let rh = wrt.finish();
							if rh != nf.hash {
								return Some(anyhow!("Written {} was different to expected (hash was {rh}, not {})", nf.path, nf.hash));
							}

							inc(&bar_new);

							None
						})
						.collect();

					if !checks.is_empty() {
						errs.lock().unwrap().extend(checks.drain(..));
						return;
					}
				});
			}
			if self.manifest.patched_files.is_empty() {
				bar_patched.done_clear();
			}
			else {
				s.spawn(|_| {
					// handle patched files
					let mut checks: Vec<_> =
						self.manifest.patched_files
							.par_iter()
							.filter_map(|pf| {
								let mut src = handle_res_parit!(File::open(self.old_root.join(&pf.path)), "Failed to open file to patch from {}", pf.path);
								let mut dst = handle_res_parit!(create_file(&self.new_root.join(&pf.path)), "Failed to create file to patch to {}", pf.path);

								// get length of src
								let src_len = handle_res_parit!(src.metadata(), "Couldn't get length of patch source file {}", pf.path).len();

								let mut src = hash::XXHashStreamer::new(&mut src);
								let mut dst = hash::XXHashStreamer::new(&mut dst);

								let blob = if let Some(t) = self.blobs_patch.get(pf.index as usize) {
									*t as usize
								}
								else {
									return Some(anyhow!("patched file {} had an out-of-range index pointing to its data", pf.path));
								};

								// get diff blob ready
								let mut diff = Cursor::new(&diff_map[blob..]);

								// apply!
								handle_res_parit!(zstddiff::apply(&mut src, &mut diff, &mut dst, src_len), "Failed to apply diff for {}", pf.path);

								let src_rh = src.finish();
								let dst_rh = dst.finish();
								if src_rh != pf.old_hash {
									return Some(anyhow!("Source {} was different to expected (hash was {src_rh}, not {})", pf.path, pf.old_hash));
								}
								if dst_rh != pf.new_hash {
									return Some(anyhow!("Written {} was different to expected (hash was {dst_rh}, not {})", pf.path, pf.new_hash));
								}

								inc(&bar_patched);

								None
							})
							.collect();

					if !checks.is_empty() {
						errs.lock().unwrap().extend(checks.drain(..));
					}
				});
			}
		});

		aggregate_errors!(errs.into_inner()?);

		as1.all_good();
		drop(as2);
		drop(as3);
		drop(as4);
		Ok(())
	}
}

/// handles initialising an in-memory applying state from disk
pub fn read_diff_from_file(path: &Path) -> anyhow::Result<ApplyingDiff> {
	let f = File::open(path).context("Failed to open file to read diff")?;

	// safety: UB if the underlying diff is modified by someone else
	// todo: is this just acceptable? do we need to lock the file (unix only) or equivalent?
	let map = unsafe { Mmap::map(&f) }?;

	let mut res = read_diff_from(&mut Cursor::new(&map))?;
	res.read = Some(map);
	Ok(res)
}

pub fn read_diff_from(reader: &mut (impl Read + Seek)) -> anyhow::Result<ApplyingDiff> {
	// checks magic bytes and version too
	let manifest = DiffManifest::read_from(&mut *reader)?;

	// create self
	let mut new_self = ApplyingDiff::default();
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
		new_self.blobs_patch.push(reader.stream_position()?);

		// read through array
		// read chunk count
		let mut count = [0u8; 8];
		reader.read_exact(&mut count).context("Failed to read diff chunk count")?;
		let count = u64::from_be_bytes(count);

		for _ in 0..count {
			// read chunk length
			let mut len = [0u8; 8];
			reader.read_exact(&mut len).context("Failed to read diff chunk length")?;
			let len = u64::from_be_bytes(len);
			// advance reader through it
			reader.seek_relative(len as i64).context("Failed to seek through diff")?;
		}
	}

	Ok(new_self)
}