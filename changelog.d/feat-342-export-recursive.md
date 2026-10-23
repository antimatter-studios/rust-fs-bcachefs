### Added

- `fs.bcachefs TARGET export --recursive PATH DEST` copies a directory tree
  through rust-fs-core's shared tree transfer, entering subvolumes and
  snapshots as `ls --recursive` does. Every item is reported as completed,
  skipped, refused or failed, and a partial tree exits 1. Existing
  directories are merged into, `--max-depth` bounds the walk, and special
  files are skipped. A file whose data cannot be read fails alone and is
  never published. `--preserve=links` recreates symlinks without following
  them and keeps hard links, a snapshot's files kept apart from the live
  subvolume's. `--preserve=times` sets the stored access and modification
  times. `--verify` reads each staged file back before publication, for
  single files too. Checked against the reference formatter's tree 33
  directories deep and the reference mount's own links and times (#342).

### Changed

- rust-fs-core is pinned at 0.3.15, the release carrying the shared tree
  transfer (#342).
