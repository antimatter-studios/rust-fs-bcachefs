### Added

- **`fs.bcachefs read` streams regular files.** `read PATH` writes raw bytes
  to stdout; `read PATH -o FILE` writes `FILE.partial` and renames it on
  success. Existing partial files are refused, and failed extraction
  preserves the destination.

### Changed

- **`cat` is an identical alias of `read`.** Both refuse symlinks with an
  error naming the target, rather than printing the target's text. Directories
  and special files are refused too.
