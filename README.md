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
| Superblock (magic, UUIDs, versions, geometry, layout, fields, checksum) | parsed; compared with the reference tools' report of every fixture |
| Btree nodes, bsets, packed bkeys | see `docs/clean-room.md` for progress and open questions |
| Inodes, dirents, extents; readdir and read | see `docs/clean-room.md` |
| Compression, encryption, multiple devices, snapshots | not implemented |
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
