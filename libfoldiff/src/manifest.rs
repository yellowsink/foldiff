use std::io::{Read, Seek};
use anyhow::{ensure, Context, Result};
use derivative::Derivative;
use rmp_serde::Deserializer;
use serde::{Deserialize, Serialize};
use zstd::Decoder;
use crate::common::{MAGIC_BYTES, VERSION_NUMBER_1_0_0_R, VERSION_NUMBER_1_1_0};

/// Messagepack manifest structure stored in the diff file
#[derive(Clone, Debug, Serialize, Deserialize, Derivative)]
#[derivative(Default)]
pub struct DiffManifest {
    #[derivative(Default(value="[0,0,0,0]"))] // invalid null default
    version: [u8; 4],
    pub untouched_files: Vec<HashAndPath>,
    pub deleted_files: Vec<HashAndPath>,
    pub new_files: Vec<NewFile>,
    pub duplicated_files: Vec<DuplicatedFile>,
    pub patched_files: Vec<PatchedFile>,
}

type HashAndPath = (u64, String);

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(crate) struct NewFile {
    pub hash: u64,
    pub index: u64,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(crate) struct DuplicatedFile {
    pub hash: u64,
    pub idx: u64, // u64::MAX == none
    pub old_paths: Vec<String>,
    pub new_paths: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(crate) struct PatchedFile {
    pub old_hash: u64,
    pub new_hash: u64,
    pub index: u64,
    pub path: String,
}

impl DiffManifest {
    pub(crate) fn read_100r(reader: impl Read) -> Result<Self> {
        let mut deserializer = Deserializer::new(reader);
        let manifest =
            DiffManifest::deserialize(&mut deserializer).context("Failed to deserialize diff format")?;

        // check version
        ensure!(
			manifest.version == VERSION_NUMBER_1_0_0_R,
			"Did not recognise version number {:x?}",
			manifest.version
		);

        Ok(manifest)
    }

    pub(crate) fn read_110(mut reader: impl Read) -> Result<Self> {
        // read compressed data length
        let mut len = [0u8; 8];
        reader.read_exact(&mut len)?;
        let len = u64::from_be_bytes(len);

        let decoder = Decoder::new(reader.take(len))?;
        let mut deser = Deserializer::new(decoder);

        DiffManifest::deserialize(&mut deser).context("Failed to deserialize diff format")
    }

    // checks the magic bytes are valid, reads the version, rewinds by 4 bytes if 1.0.0-r, and returns it.
    // does not check that raw manifests contain the 1.0.0-r version, you must check that yourself.
    // for compressed manfests, verifies that the version is supported by this software.
    pub(crate) fn verify_and_read_ver(mut reader: impl Read+Seek) -> Result<[u8; 4]> {
        let mut magic = [0u8, 0, 0, 0];
        reader
            .read_exact(&mut magic)
            .context("Failed to read magic bytes from diff")?;
        ensure!(
			magic == MAGIC_BYTES,
			"Magic bytes did not match expectation ({magic:x?} instead of 'FLDF')"
		);

        // check next byte
        let mut ver = [0u8; 4];
        reader.read_exact(&mut ver)?;
        if ver[0] == 0 {
            // null byte, we are using a compressed manifest
            // check version
            ensure!(
				ver == VERSION_NUMBER_1_1_0,
				"Did not recognise version number {:x?}",
				ver
			);
            Ok(ver)
        }
        else {
            // we just read into a raw manifest - 1.0.0-r
            reader.seek_relative(-4)?;
            Ok(VERSION_NUMBER_1_0_0_R)
        }
    }

    pub fn read_from(mut reader: impl Read+Seek) -> Result<Self> {
        let ver = Self::verify_and_read_ver(&mut reader)?;

        if ver == VERSION_NUMBER_1_0_0_R {
            Self::read_100r(reader)
        }
        else {
            Self::read_110(reader)
        }
    }
}
