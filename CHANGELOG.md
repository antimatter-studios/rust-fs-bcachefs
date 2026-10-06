# Changelog

Notable changes to `rust-fs-bcachefs`, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This is a `0.x`
crate, so the **minor** is the compatibility boundary: a minor bump may break
API, a patch never does.

## [Unreleased]

### Added

- **A read-only spike of a clean-room bcachefs reader.** The superblock is
  parsed and checksummed; btree nodes are read, checksummed and walked;
  inodes, directory entries and extents are decoded; files read back byte
  for byte, with crc32c, crc64 and xxhash data checksums and lz4, zstd and
  gzip compression. Every structure is compared with the reference tools'
  view of eight fixture sets.
- **A C ABI** (`fs_bcachefs_mount`, `_stat`, `_readdir`, `_read_file`,
  `_umount`) and the `fs.bcachefs` tool (`info`, `ls`, `cat`).
- **Fuzz targets** for the superblock and btree-node decoders, with a
  corpus of real blocks replayed on every pull request.
- **Fixtures made by the reference formatter in the Linux test VM**, each with
  a JSON record of what the reference tools say is in it.
- **docs/clean-room.md**, the provenance of every fact about the format.
- **Inline data**: small files a mounted filesystem stores inside the
  extents btree read back byte for byte.
- **An aged fixture**, mounted and aged by the reference implementation in
  the test VM, with hard links, renames, deletions, overwrites, sparse and
  fragmented files; every file and listing is compared with the mount's view.

### Fixed

- **An uncleanly unmounted filesystem is refused** instead of being read
  from roots that are stale until its journal is replayed.
- **Link counts** are reported as a mount reports them (`Inode::link_count`).
