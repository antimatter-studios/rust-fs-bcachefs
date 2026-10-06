# rust-fs-bcachefs

A pure-Rust, **read-only** bcachefs reader with a C ABI (`fs_bcachefs_*`),
written clean-room: from prose documentation and black-box observation of
images, never from the GPL implementation. MIT-licensed.

> **Status: spike.** This repository is a timeboxed feasibility study. See
> `## Status` for what reads today and `docs/clean-room.md` for how every fact
> about the format was learned.

## Status

| Structure | State |
|---|---|
| Superblock (magic, UUIDs, versions, geometry, layout, fields, members, btree roots; crc32c/crc64/xxhash checksums) | read; every field compared with the reference printer (`tests/oracle_superblock.rs`) |
| Btree nodes, bsets, packed and unpacked keys, interior nodes | read and checksummed; every key of the inodes, dirents and extents btrees compared with the reference lister (`tests/oracle_tree.rs`) |
| Inodes (v3), dirents, extents; lookup, readdir, read | every path, listing, inode and file SHA-256 compared with the formatted tree (`tests/oracle_fs.rs`) |
| Data checksums crc32c, crc64, xxhash; lz4, zstd, gzip | read and verified |
| C ABI (`include/fs_bcachefs.h`): mount, stat, readdir, read_file | `tests/capi.rs` |
| `fs.bcachefs` CLI: info, ls, cat | built with `--features cli` |
| Unclean filesystems (journal replay), snapshots, encryption, multiple devices, erasure coding, reflink, inline data, xattrs | not implemented; see `docs/clean-room.md` open questions |
| Writing | out of scope: this is a reader |

## Clean room

bcachefs's kernel code and userspace tools are GPL-2.0. Nothing from them is
in this repository: not copied, not translated, not linked, not a dependency.
The reference tools are used only as a test oracle, run inside a disposable
Linux VM, and are referred to by role. `docs/clean-room.md` records every
source used and what it told us, so the provenance of each fact is auditable.

## Test contract

- **unit** (`chore test:unit`): no tool, no fixture, no VM; debug profile with
  overflow checks on.
- **oracle** (`chore test:oracle`): reads every fixture the reference formatter
  made in the Linux VM (`chore fixtures`) and compares this crate's reading
  with what the reference tools recorded about it. A missing fixture fails the
  test; nothing skips.

Every tier runs through rust-fs-core's `tier.sh`, quiet and output-budgeted,
with an executed-test floor from `test-floor.sh`.

## Building

```sh
chore siblings     # ../rust-fs-core and ../fs-linux-test-harness at pinned refs
cargo build
chore staticlib    # dist/libfs_bcachefs.a and its headers
```

## Licence

MIT. See `LICENSE`.
