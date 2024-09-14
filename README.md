# Foldiff

[![License: OQL](https://badgers.space/badge/License/OQL/pink)](https://oql.avris.it/license/v1.2)

A general purpose diffing tool that operates on folders of mixed text/binary files.

## Motivation

I like to keep frequent backup of data.
I'm also a bit of a data hoarder and take exports of my data from places periodically.

This leads to a bit of an issue when I go to upload these backups to my cloud storage—they're huge!
My first attempt to solve this was by tarring the old and new folders with
[tar-sorted](https://github.com/zholos/tar-sorted),
and running the results through [xdelta3](https://github.com/jmacd/xdelta).

This definitely worked, but not *well*. It often made unnecessarily huge diffs even once compressed.
I knew we could do better, so I sat down and made it myself.

Foldiff: a diffing program that takes two very similar directories and makes a *very* efficient diff
file that can convert the old folder into the new one.

This way I can compress and store the first backup, then I can just keep storing tiny diffs.
Nice.

I hope you find this as useful as I have :)

## Usage

Create a diff:
```sh
foldiff diff old-files new-files diff.fldf
```

Apply a diff:
```sh
foldiff apply old-files diff.fldf new-files
```

Check if two folders are the same
```sh
foldiff verify old-files new-files
```

Check if the first folder is in the state expected by the diff,
and the second folder is equivalent to what that diff would output
```sh
foldiff verify old-files new-files diff.fldf
```

Symlinks are not supported.
Empty folders are not stored.

## General principle

The method of diffing is as follows
- hash all files (both old and new)
- perform basic file type inference on all files (both old and new)
- files that are identical path and content between new and old are noted down but from then on ignored
- subsequent files with identical hashes are treated as noted as duplicates of the first (old & new)
- when old and new structures have files that share hashes, store that as a rename/copy/move operation
 * (store list of old and list of new files with that hash)
- for files without hash matches, where both folders have a file with that path:
 * run the binary diffing algorithm (below) on that file, to generate a diff, and store that
- for files without hash matches, where only the old folder contains that path
 * store that path to be deleted
- for files without hash matches, where only the new folder contains that path
 * store that file as new, compressing with zstd
- write the manifest listing paths, hashes, etc, into the file
- sort the list of diffs by file type, then by the name sorting algorithm (see below), and place into the file
- sort the list of new files by type, then name, and write

Then to apply
- read the manifest from the stream
- for files that can just be kept, or can be copied to a set of new locations (and possibly delete some old locations), apply that
- apply simple file deletions
- create newly added files
- apply all xdelta3 diffs

The name sorting algorithm is:
- split the name by `/` slashes
- reverse the order of the segments
- sort by each segment in turn (e.g. equivalent to re-joining the segments and sorting)

Binary diffing algorithm:
- Calculate the minimum number of chunks required to split the old file into chunks of MAX 2GB
- Split both the old and new file evenly into that many chunks
- For each pair of chunks, use the old chunk as a dictionary to compress the new chunk with zstd, in long mode.
- Store the zst chunks

To apply the binary diff:
- Split the old file into the same chunks
- Decompress each diff using the old chunk as the dictionary with zstd
- Concatenate the decompressed chunks

## Stored file type

all numbers are stored in big-endian, because it is the correct choice :)

- magic bytes, ASCII 'FLDF'
- A messagepack object
  - version number, [u8, u8, u8, 'r'|'b'|'a'], 1.0.0-r
  - untouched files (list of following:)
    * path
    * [XXH64](https://xxhash.com/) hash
  - delete files (list of following:)
    * XXH64 hash
    * path in old folder
  - new files (list of following:)
    * new XXH3 hash
    * u64 index into new array
    * path
  - duplicated files (list of following:)
    * XXH64 hash
    * u64 index into new array, u64::MAX if not necessary
    * list of paths in old folder
    * list of paths in new folder
  - patch files (list of following:)
    * old XXH64 hash
    * new XXH64 hash
    * u64 index into patch array
    * path
- new files:
  * u64 number of elements
  * repetition of:
    * u64 size of blob
    * binary blob of compressed zstd data
- patch files:
  * u64 number of diffs
  * repetition of:
    * u64 number of chunks in this diff
    * repetition of:
      * u64 length of diff
      * binary blob of compressed diff data

## Progress

- [x] Diffing
  * [x] Working diff generator
  * [x] Does not keep blobs in memory
  * [ ] Multi-threaded (zstd is multithreaded but scanning, diffing, and compression are not)
- [x] Applying
  * [x] Working application
  * [x] Does not keep blobs in memory
  * [x] Multi-threaded
- [x] Verifying
  * [x] Folder equality
  * [x] Diff checking
    - [ ] Checks for unexpected files