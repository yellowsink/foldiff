use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use anyhow::{Context, Result};
use rmp_serde::{Deserializer, Serializer};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Diff {
    magic: u32,   // ASCII 'FLDF'
    version: u32, // 0x24 09 06 01

    untouched_files: Vec<HashAndPath>,
    deleted_files: Vec<HashAndPath>,
    new_files: Vec<NewFile>,
    duplicated_files: Vec<DuplicatedFile>,
    patched_files: Vec<PatchedFile>,

    // todo: do not store the blobs in memory you fuckin idiot LMAO
    blobs_new: Vec<ByteBuf>,
    blobs_patch: Vec<PatchBlob>,
}

// untouched files, deleted files
#[derive(Clone, Debug, Serialize, Deserialize)]
struct HashAndPath(u64, String);

#[derive(Clone, Debug, Serialize, Deserialize)]
struct NewFile {
    hash: u64,
    index: u64,
    path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DuplicatedFile {
    hash: u64,
    old_paths: Vec<String>,
    new_paths: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PatchedFile {
    old_hash: u64,
    new_hash: u64,
    index: u64,
    path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PatchBlob(Vec<ByteBuf>);

impl Diff {
    fn new() -> Self {
        Self::default()
    }
    
    fn write_to(&self, writer: &mut impl Write) -> Result<()> {
        let mut serializer = Serializer::new(writer);

        self.serialize(&mut serializer).context("Failed to serialize diff format into file")
    }

    fn write_to_file(&self, path: &Path) -> Result<()> {
        // create file
        let mut f = File::create_new(path).context("Failed to create file to save diff")?;

        self.write_to(&mut f)
    }
    
    fn create_from(reader: &mut impl Read) -> Result<Self> {
        let mut deserializer = Deserializer::new(reader);
        
        Self::deserialize(&mut deserializer).context("Failed to deserialize diff format from file")
    }
    
    fn create_from_file(path: &Path) -> Result<Self> {
        let mut f = File::open(path).context("Failed to open file to read diff")?;
        
        Self::create_from(&mut f)
    }
    
    // TODO: put the functions to add files here
}

impl Default for Diff {
    fn default() -> Self {
        Self {
            magic: 0x46_4c_44_46,
            version: 0x24_09_06_01,
            untouched_files: Vec::new(),
            deleted_files: Vec::new(),
            new_files: Vec::new(),
            duplicated_files: Vec::new(),
            patched_files: Vec::new(),
            blobs_new: Vec::new(),
            blobs_patch: Vec::new(),
        }
    }
}