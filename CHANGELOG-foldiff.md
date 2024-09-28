## to-do
- multithread diffing
   * diffing (maybe)
   * compressing (maybe)
- `foldiff stats` after diffing, before applying, and standalone
- replace `anyhow` with custom error types
- write custom threading utilities

## pending
- `foldiff upgrade` - upgrade older manifests to new ones
- move core `foldiff` functionality to `libfoldiff`
  * significant refactors
  * decouple logic from `indicatif` and `cliutils`

## 1.2.0
- switch to FLDF v1.1.0
- diff versioning handling to allow still reading FLDF 1.0.0-r

## 1.1.0
- Force windows to use the `/` path separator over `\` for portability.

## 1.0.1
- Fix diffing not working with relative paths for inputs

## 1.0.0
- yeah.