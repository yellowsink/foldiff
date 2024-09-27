## to-do
- store file attributes

## v1.1.0
- remove the version field from the manifest
- add a null byte and 3-byte version number after the magic bytes
- add an 8-byte byte length field after the version number for the compressed manifest blob
- the manifest is now zstd-compressed

## 1.0.0-r:
- initial format