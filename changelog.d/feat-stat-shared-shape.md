### Changed

- **`fs.bcachefs stat` reports the shape every `fs.<fs> stat` shares.** The
  shared keys come first, in order: `path`, `inode`, `type`, `mode`, `uid`,
  `gid`, `nlink`, `size`, `allocated_bytes`, then the access, modification,
  change and creation times as signed seconds since the epoch, each with
  its nanoseconds. A symlink adds its `target` and is not followed; a
  non-UTF-8 path is resolved as bytes and adds `path_hex`. The inode's
  snapshot, sectors, named flags, generation and data checksum and
  compression options are under `bcachefs`. The `ino`, `sectors` and
  `atime_ns`/`mtime_ns`/`ctime_ns` keys are gone: `inode`,
  `bcachefs.sectors` and the seconds plus nanoseconds replace them. Every
  field is checked against the reference mount's own lstat and the
  reference lister (#251).
