### Added

- **The C ABI writes through an fs_core device** (#188):
  `fs_bcachefs_mount_rw_with_fs_core_device` opens a write handle on a
  file, a slice or host callbacks, as `fs_bcachefs_mount_with_fs_core_device`
  does for reading; a device that cannot be written is refused at open
  with EROFS.
- **Every C ABI failure names its errno** (#188): `fs_bcachefs_last_errno`
  gives the errno of the thread's most recent failed call, read or write:
  ENOENT for a path that is not there, EINVAL for a readlink of what is not
  a symlink, ERANGE for a buffer too small, ENOATTR (ENODATA on Linux) for
  a missing extended attribute, and the writes' own.
- **Inode times are read through the C ABI** (#188):
  `fs_bcachefs_stat_times` gives a path's access, modification and change
  times as seconds and nanoseconds since the epoch.
- **Capacity and free space are reported** (#188): `fs_bcachefs_statfs`
  and `Filesystem::usage` give the member's buckets and the freespace
  btree's runs in bytes, as the reference tools count them.

### Fixed

- **The C ABI's errnos are the platform's numbers** (#188): the writes
  returned Linux's numbers everywhere, so on macOS a non-empty `rmdir` came
  back as -39 rather than -ENOTEMPTY (66), and ENOTSUP as -95 rather than
  45.
