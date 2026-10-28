### Added

- `fs.bcachefs TARGET export --preserve=xattrs PATH DEST` carries each
  file's and directory's extended attributes, name and value, to the host,
  for a tree with `--recursive` and for a single file, through
  rust-fs-core's shared attribute capability. An attribute the host cannot
  hold, such as `trusted.*` without privilege, refuses its item by name
  after the contents are published, and the export exits nonzero; nothing
  is dropped. Attributes need a local DEST, so standard output is refused.
  Checked against the attributes the reference mount set and listed on the
  aged image (#442).
