# rust-fs-bcachefs

A pure-Rust bcachefs driver with a C ABI (`fs_bcachefs_*`) and command-line
tools (`fs.bcachefs`, `fsck.bcachefs`), written clean-room: from prose
documentation and black-box observation of images, never from the GPL
implementation. MIT-licensed.

Reading is the supported use. Writing is being built behind the `write`
cargo feature, off by default: every image it writes in the tests passes the
reference checker and reads back through the reference implementation's
mount, but it is new, partial, and has not been used on data anyone cares
about.

## Status

| Area | State | Checked by |
|---|---|---|
| Superblock: identity, geometry, layout, fields, members, btree roots; crc32c/crc64/xxhash | read | every field against the reference printer (`tests/oracle_superblock.rs`) |
| Btree nodes: bsets, packed and unpacked keys, interior nodes, nodes of many bsets | read through a lazy cursor (`btree::Cursor`) | every key against the reference lister (`tests/oracle_tree.rs`); node reads bounded (`tests/oracle_cursor.rs`) |
| Inodes, dirents (found at their SipHash slot), extents, inline data, xattrs | read | every path, listing, inode, xattr and SHA-256 against the tree the reference wrote (`tests/oracle_fs.rs`, `oracle_aged.rs`, `oracle_xattr.rs`) |
| Data: crc32c, crc64, xxhash; lz4, zstd, gzip | read and verified | as above |
| Unclean filesystems | read through an in-memory replay of the journal; the device is never written | against the reference's own replay (`tests/oracle_journal.rs`) |
| Encrypted and multi-device filesystems | refused by name | `tests/oracle_refused.rs` |
| Snapshots and subvolumes, reflink | not read: no fixture can be made without a kernel with bcachefs | |
| `fsck.bcachefs`: check-only (`check::check`) — btree nodes, key order, directory entries, link counts, extents, data checksums; fsck(8) exit status; never writes | works | clean and deliberately damaged images against the reference checker's verdict, in the guest (`tests/check_oracle.rs`, `oracle_check.rs`) |
| `fs.bcachefs`: `info`, `ls`, `cat`, `stat`, `tree` | built with `--features cli` | `tests/cli/` against the installed tools |
| C ABI (`include/fs_bcachefs.h`): mount (path or fs_core device), volume info, stat, readdir and a directory iterator, read, readlink, listxattr, getxattr | works | `tests/capi.rs`, `tests/oracle_capi.rs`, and the header compiled as C |
| Writing (`--features write`), in place: small (inline) files created and rewritten, mkdir, unlink, rmdir, rename | experimental | every written image passes the reference checker (`fsck -n`) and reads back byte for byte through the reference implementation's mount, in the guest (`tests/write_oracle.rs`) |
| Writing: allocated (non-inline) data, freeing space, journalled commits, links and attributes, splitting full nodes | in review, one pull request each (road to read-write) | |

A create that would need a btree node with no room left is refused before
anything is written, until splitting full nodes lands. A kernel mount has not
been used as a judge: the harness has no kernel whose bcachefs module reads
what the reference tools write, so the reference implementation's own mount
stands in for it.

## Clean room

bcachefs's kernel code and userspace tools are GPL-2.0. Nothing from them is
in this repository: not copied, not translated, not linked, not a dependency.
The reference tools are used only as a test oracle, run inside a disposable
Linux VM, and are referred to by role. `docs/clean-room.md` records every
source used and what it told us, so the provenance of each fact is auditable;
what could not be learned that way is listed there as an open question.

## Test contract

- **unit** (`chore test:unit`): no tool, no fixture, no VM; debug profile with
  overflow checks on; and the fuzz targets' decoders with checksums switched
  off (`--features fuzzing`).
- **oracle** (`chore test:oracle`): every fixture the reference tools made in
  the Linux VM (`chore fixtures`), compared with what they recorded about it.
  A missing fixture fails the test; nothing skips.
- **vm** (`chore test:vm`): the same, compiled and run inside the guest, plus
  the tests that need the reference tools there: the write path and the
  checker against the reference checker.
- **cli** (`chore test:cli`): the installed tools, found on PATH.
- **coverage** (`chore coverage`) and **semver** (`chore check:semver`).

Every tier runs through rust-fs-core's `tier.sh`, quiet and output-budgeted,
with an executed-test floor from `test-floor.sh`.

## Building

```sh
chore siblings     # ../rust-fs-core and ../fs-linux-test-harness at pinned refs
cargo build
chore staticlib    # dist/libfs_bcachefs.a and its headers
chore cli:install  # the tools, staged in tmp/cli
```

## Changelog

The latest releases; every release, with the reasoning behind each change, is in [CHANGELOG.md](CHANGELOG.md).

There is no release yet. What the first one will carry is under
[Unreleased](CHANGELOG.md#unreleased) in CHANGELOG.md.

## Licence

MIT. See `LICENSE`.
