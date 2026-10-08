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

Reading is supported for single-device, unencrypted filesystems, clean or
not, with every checksum and compression the reference writes; `fsck.bcachefs`
checks without repairing; writing is experimental. **[docs/features.md](docs/features.md)
is the full list**: every feature, its state (supported, experimental,
partial, refused, upcoming, or unobservable clean-room), the release it
shipped in, its tracking issue and the test that checks it. Every pull
request that changes behaviour updates it.

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
