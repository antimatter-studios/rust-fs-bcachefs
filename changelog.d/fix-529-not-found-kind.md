### Changed

- **`fs.bcachefs` reports a missing path with the `not_found` error kind
  (#404).** `ls`, `ls -R`, `stat`, `read`, `export` and
  `export --recursive` given a path that is not there, or one whose parent
  is not there, still exit 1 and now carry `"kind": "not_found"` beside
  `code` and `error` in the JSON error. A script can match that across
  drivers, whatever the message says. The library's `Error::NotFound` is
  that kind, as it is the C ABI's ENOENT. A structure missing at the open,
  where no path is looked up, stays an ordinary failure.
- **Built on rust-fs-core 0.4.2**, for `CliError::not_found`.
