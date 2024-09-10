use crate::zstddiff;
use anyhow::{bail, ensure, Context, Result};
use rmp_serde::{Deserializer, Serializer};
use serde::{de::IgnoredAny, Deserialize, Serialize};
use std::fs::File;
use std::io::{copy, Read, Seek, Write};
use std::path::{Path, PathBuf};
use crate::hash;

static VERSION_NUMBER: [u8; 4] = [0x24, 0x09, 0x06, 0x01]; // 2024-09-06 r1

/// In-memory representation of a folder diff.
/// This struct serializes via messagepack to the manifest chunk of the on-disk format,
/// but does not include the format header or binary blobs.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Diff<R: Read+Seek = std::io::Cursor<&'static [u8]>> {
	// default type of R for convenience, as this is only used for applying

	untouched_files: Vec<HashAndPath>,
	deleted_files: Vec<HashAndPath>,
	new_files: Vec<NewFile>,
	duplicated_files: Vec<DuplicatedFile>,
	patched_files: Vec<PatchedFile>,

	// do not store the blobs in memory, store instructions to serialize them or find them
	#[serde(skip)]
	working_data: WorkingData<R>,
	#[serde(skip)]
	version: [u8; 4], // 0x24 09 06 01
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

/// the part of the diff that is tracking useful data for diffing/applying
#[derive(Clone, Debug)]
enum WorkingData<R: Read+Seek> {
	Diffing(WorkingDiffingData),
	Applying(WorkingApplyingData<R>),
}

// ugh.
impl<R: Read+Seek> Default for WorkingData<R> {
	fn default() -> Self {
		WorkingData::Diffing(Default::default())
	}
}

#[derive(Clone, Debug, Default)]
struct WorkingDiffingData {
	blobs_new: Vec<PathBuf>,
	blobs_patch: Vec<(PathBuf, PathBuf)>, // old, new
	old_root: PathBuf,
	new_root: PathBuf,
}

#[derive(Clone, Debug, Default)]
struct WorkingApplyingData<R: Read+Seek> {
	blobs_new: Vec<u64>,   // offset into diff file
	blobs_patch: Vec<u64>, // offset into diff file
	read: R, // the diff file stream
	old_root: PathBuf,
	new_root: PathBuf,
}

impl Diff {
	fn new() -> Self {
		Self::default()
	}

	/// handles finalising an in-memory diffing state to disk
	fn write_to(&self, writer: &mut (impl Write + Seek)) -> Result<()> {
		let working_data = if let WorkingData::Diffing(tmp) = &self.working_data {
			tmp
		} else {
			bail!("Cannot call write_to on a Diff with an Applying() working_data");
		};

		// write magic bytes and version number
		writer.write_all("FLDF".as_bytes())?;
		writer.write_all(&VERSION_NUMBER)?; // 2024-09-06 r01

		let mut serializer = Serializer::new(&mut *writer); // lol re-borrowing is goofy but sure
		self
			.serialize(&mut serializer)
			.context("Failed to serialize diff format into file")?;
		drop(serializer); // this drops here anyway, but is load-bearing, so make it explicit

		// write new files
		writer.write_all(&(working_data.blobs_new.len() as u64).to_be_bytes())?;

		for path in &working_data.blobs_new {
			let mut f =
				File::open(path).context("Failed to open file while copying newly added files")?;
			let len = f.metadata()?.len(); // this better be accurate lol

			writer.write_all(&len.to_be_bytes())?;
			let bytes = copy(&mut f, writer)?;

			if bytes != len {
				bail!(
					"Bytes written did not match expected file length whn writing newly added file '{}'",
					path.to_str().unwrap_or("<invalid unicode>")
				);
			}
		}

		// write patches
		writer.write_all(&(working_data.blobs_patch.len() as u64).to_be_bytes())?;
		//writer.write_all(&0u64.to_be_bytes())?;

		// perform diffing
		for (old_p, new_p) in &working_data.blobs_patch {
			let mut old = File::open(old_p).context("Failed to open old file for diffing")?;
			let mut new = File::open(new_p).context("Failed to open new file for diffing")?;

			let ol = old.metadata()?.len();
			let nl = new.metadata()?.len();

			zstddiff::diff(&mut old, &mut new, &mut *writer, None, Some(ol), Some(nl))
				.context("Failed to perform diff")?;
		}

		Ok(())
	}

	fn write_to_file(&self, path: &Path) -> Result<()> {
		// create file
		let mut f = File::create_new(path).context("Failed to create file to save diff")?;

		self.write_to(&mut f)
	}

	/// handles initialising an in-memory applying state from disk
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

		// check version
		let mut version = [0u8, 0, 0, 0];
		reader.read_exact(&mut version)?;
		ensure!(
			version == VERSION_NUMBER,
			"Did not recognise version number {version:x?}"
		);

		// deserialize msgpack data
		// this better understand when to stop reading lol
		let mut deserializer = Deserializer::new(&mut *reader);
		let mut deserialized =
			Self::deserialize(&mut deserializer).context("Failed to deserialize diff format")?;
		drop(deserializer); // this drops here anyway, but is load-bearing, so make it explicit

		// create working data
		let mut working_data = WorkingApplyingData::default();

		let mut new_blob_count = [0u8, 0, 0, 0, 0, 0, 0, 0];
		reader
			.read_exact(&mut new_blob_count)
			.context("Failed to read new file count")?;
		let new_blob_count = u64::from_be_bytes(new_blob_count);

		for _ in 0..new_blob_count {
			// keep track of the offset
			working_data.blobs_new.push(reader.stream_position()?);

			// read blob length
			let mut len = [0u8, 0, 0, 0, 0, 0, 0, 0];
			reader
				.read_exact(&mut len)
				.context("Failed to read new file length")?;
			let len = u64::from_be_bytes(len);

			// keep track of the offset
			working_data.blobs_new.push(reader.stream_position()?);
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
			working_data.blobs_new.push(reader.stream_position()?);

			// read through array
			// this is not that efficient but oh well
			let mut deser = Deserializer::new(&mut *reader);
			// lol name collision
			serde::Deserializer::deserialize_any(&mut deser, IgnoredAny)
				.context("Failed to read through patched file data")?;
		}

		// set version number and working data
		deserialized.version = version;
		deserialized.working_data = WorkingData::Applying(working_data);
		Ok(deserialized)
	}

	fn read_from_file(path: &Path) -> Result<Self> {
		let mut f = File::open(path).context("Failed to open file to read diff")?;

		Self::read_from(&mut f)
	}

	// TODO: finish diffing functionality
	// TODO: applying functionality

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

	/// handles a file in the 'old' folder while in diffing state
	/// this assumes the passed file does not currently exist in our diff state
	fn diff_old_file(path: &Path) -> Result<()> {
		todo!()
	}
}

impl Default for Diff {
	fn default() -> Self {
		Self {
			version: [0x24, 0x09, 0x06, 0x01],
			working_data: Default::default(),

			untouched_files: Vec::new(),
			deleted_files: Vec::new(),
			new_files: Vec::new(),
			duplicated_files: Vec::new(),
			patched_files: Vec::new(),
		}
	}
}
