# Foldiff

A general purpose diffing tool that operates on folders of mixed text/binary files.

## Usage

```sh
./foldiff diff old-files new-files diff.tar.zst
```

```sh
./foldiff apply old-files diff.tar.zst new-files
```

Foldiff always outputs tar.zst files.

When diffing, pass `-Z 1` to `-Z 22` to set the zstd compression setting.

Symlinks are not supported.

## General principle

The method of diffing is as follows
- hash all files (both old and new)
- perform basic file type inference on all files (both old and new)
- subsequent files with identical hashes are treated as noted as duplicates of the first (old & new)
- when old and new structures have files that share hashes, store that as a rename/copy/move operation
 * (store list of old and list of new files with that hash)
- for files without hash matches, where both folders have a file with that path:
 * run xdelta3 on that file, to generate a diff, and store that
- for files without hash matches, where only the old folder contains that path
 * store that path to be deleted
- for files without hash matches, where only the new folder contains that path
 * store that file as new
- write the manifest listing paths, hashes, etc, into the tar stream
- sort the list of diffs by file type, then by name (using name sorting algo below), and write into tar
- sort the list of new files by type, then name, and write into tar
- tar stream runs through zstd

Then to apply
- read the manifest from the stream
- for files that can just be kept, or can be copied to a set of new locations (and possibly delete some old locations), apply that
- apply simple file deletions
- create newly added files
- apply all xdelta3 diffs

The name sorting algorithm mentioned is:
- split the name by `/` slashes
- reverse the order of the segments
- sort by each segment in turn (e.g. equivalent to re-joining the segments and sorting)
