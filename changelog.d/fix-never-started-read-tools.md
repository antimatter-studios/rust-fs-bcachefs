### Fixed

- The read tools say what an image is that the reference formatter left to its first start, in place of `not implemented: no journal_v2 field`. `get` reports its superblock, with the new `bcachefs.started` false and `free_bytes` null, since nothing records free space until a start. A verb that needs a directory, and a write verb, refuses with exit 4, saying the filesystem has not been started, and the C ABI's mount refuses with `ENOTSUP`, as the reference holds no directory on such an image until a start makes one.

### Added

- `bcachefs.started` in `fs.bcachefs get`, false on a filesystem nothing has started. `Superblock::never_started` is public.
