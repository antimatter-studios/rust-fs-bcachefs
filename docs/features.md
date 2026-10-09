# Features

What this driver does today, what it refuses, and what is coming. **Every
pull request that adds, fixes or removes behaviour updates its row here, in
the same pull request** (AGENTS.md). The provenance of each on-disk fact is
in [clean-room.md](clean-room.md); the reasoning behind each change is in
[CHANGELOG.md](../CHANGELOG.md).

**Since** is the release a row's current state shipped in. There is no
release yet, so everything is **Unreleased** and will ship as 0.3.0; the
pull request number says where it landed. **Tracking** names the issue for
anything not finished.

States:

- **Supported**: works, and is checked against the reference tools.
- **Experimental**: works in every test, but is new. The `write` feature is
  experimental as a whole.
- **Partial**: works for part of the case, and the row says which part.
- **Refused**: recognised and refused by name, rather than misread.
- **Not supported**: neither read nor refused by name.
- **Upcoming**: an open issue with a plan.
- **Unobservable**: blocked because the reference tools cannot make an image
  that shows it, so it cannot be learned clean-room.

## Reading

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| Superblock: identity, geometry, layout, fields, members, btree roots | Supported | Unreleased (#1) | | `oracle_superblock.rs` |
| Superblock copies: the highest `seq` wins, the layout copy is the fallback | Supported | Unreleased (#66) | | `oracle_superblock_copies.rs` |
| Btree nodes: bsets, packed and unpacked keys, interior nodes, many bsets | Supported | Unreleased (#1, #40) | | `oracle_tree.rs`, `oracle_formats.rs` |
| Packed key formats of every width the reference writes (whole bytes) | Supported | Unreleased (#90) | | `oracle_formats.rs` |
| Node header flags: btree id (16 and above too) and level | Supported | Unreleased (#92) | | `oracle_node_flags.rs` |
| Lazy btree cursor: a lookup reads only the nodes on its path | Supported | Unreleased (#40, #71) | | `oracle_cursor.rs`, `oracle_cursor_end.rs` |
| Inodes, every field by name | Supported | Unreleased (#93) | | `oracle_inode_fields.rs` |
| Directory entries at their hash slot, SipHash and crc32c directories | Supported | Unreleased (#88) | | `oracle_fs.rs`, `oracle_strhash.rs`, `oracle_collisions.rs` |
| Colliding directory entries (hash run, `hash_whiteout`) | Supported | Unreleased (#88) | | `oracle_collisions.rs` |
| crc64-hashed directories | Partial: found by scanning the directory | Unreleased (#72) | open question 17 | |
| Extents, inline data | Supported | Unreleased (#1) | | `oracle_fs.rs`, `oracle_aged.rs` |
| Ranged reads (a window of a file) | Supported | Unreleased (#70) | | `oracle_read_range.rs` |
| Data checksums: crc32c, crc64, xxhash | Supported | Unreleased (#1) | | `oracle_fs.rs` |
| Compression: lz4, zstd, gzip | Supported | Unreleased (#1) | | `oracle_fs.rs` |
| Background compression (`reconcile` extent entry) | Supported | Unreleased (#75) | | `oracle_bgcompress.rs` |
| Extent pointers checked against the allocator's generations | Supported | Unreleased (#69) | #94 | `oracle_pointers.rs` |
| Error extents, extent whiteouts, older inode encodings | Supported | Unreleased (#68) | | `oracle_refused_local.rs` |
| Extended attributes, user and trusted namespaces | Supported | Unreleased (#18) | | `oracle_xattr.rs` |
| Unclean filesystems, through an in-memory journal replay | Supported | Unreleased (#17) | | `oracle_journal.rs` |
| Journal sequence blacklist | Supported | Unreleased (#67) | | `oracle_blacklist.rs` |
| crc128 extent entries (extents over 512 sectors), partly overwritten ones too | Supported | Unreleased (#97) | | `oracle_extent_entries.rs` |
| Poisoned extents (flags entry): reads fail with an I/O error, as the reference's do | Supported | Unreleased (#97) | | `oracle_extent_entries.rs` |
| Stripe pointers (erasure coding, several devices) | Refused by name | Unreleased (#97) | | `src/extent.rs` unit tests |
| Encrypted filesystems | Refused | Unreleased (#19) | | `oracle_refused.rs` |
| Multi-device filesystems | Refused | Unreleased (#19) | | `oracle_refused.rs` |
| A second snapshot, overlapping extents, unknown checksum types | Refused | Unreleased (#65) | | `oracle_refused_local.rs` |
| Snapshots and subvolumes (a second snapshot is refused, #65) | Not supported | | #12; Unobservable through the reference mount (no subvolume command works through it); the reference key editor fabricates snapshot keys, but no whole snapshot yet passes the reference checker (#114) | `oracle_probes.rs` |
| Reflinked extents (`reflink_p`): refused by the reader and the writer, named by the checker | Refused | Unreleased (#100) | #7; Unobservable through the reference mount: no route makes a reflink (FICLONE, FICLONERANGE, `copy_file_range`, FIDEDUPERANGE); the reference's `kvdb` editor is the next route | `oracle_refused_local.rs`, `oracle_probes.rs` |
| Casefolded directories (`--casefold`): names listed as given, found in any case, non-ASCII included (Unicode full folding and NFD) | Supported | Unreleased (#98, #126) | folding tables are a newer Unicode than the reference's 12.1 (open question 18) | `oracle_casefold.rs` |
| Per-inode options (compression, checksum, replicas, ...) | Partial: fields read, effects unknown | Unreleased (#93) | #81; Unobservable: neither the reference mount nor its offline editor sets one (#99) | `oracle_inode_fields.rs` |

## Checking

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| `fsck.bcachefs`, check-only: nodes, key order, dirents, link counts, extents, data checksums; fsck(8) exit status | Supported | Unreleased (#39) | | `check_oracle.rs`, `oracle_check.rs` |
| Damaged superblock copies reported | Supported | Unreleased (#66) | | `tests/cli/test-fsck.sh` |
| Repair | Not supported | | | |

## Writing (`--features write`)

Every image a write test makes passes the reference checker (`fsck -n`) and
reads back byte for byte through the reference implementation's mount
(`write_oracle.rs`, in the guest).

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| Create a file in an existing directory | Experimental | Unreleased (#29) | | `write_oracle.rs` |
| A create while the inode cursor's next number is in use: the next free number is taken | Experimental | Unreleased (#117) | how the reference picks one is unobserved (#107) | `oracle_write_local.rs`, `write_oracle.rs` |
| mkdir, unlink, rmdir, rename | Experimental | Unreleased (#30) | | `write_oracle.rs` |
| rename over an existing file, and a directory moved to another directory | Experimental | Unreleased (#118) | renaming over a directory is refused (#104) | `oracle_write_local.rs`, `write_oracle.rs` |
| File data inline or in extents, as the reference lays it out | Experimental | Unreleased (#86) | | `oracle_inline_limit.rs`, `write_oracle.rs` |
| Data in allocated buckets, freed on unlink and rewrite | Experimental | Unreleased (#35, #36) | | `write_oracle.rs` |
| Data on blocks larger than 512 bytes | Experimental | Unreleased (#89) | | `oracle_write_local.rs`, `write_oracle.rs` |
| Commits through the journal | Experimental | Unreleased (#37) | | `write_oracle.rs` |
| A journal left for replay is continued by the next writing session (`Writer::open_journalled`) | Experimental | Unreleased (#46) | | `oracle_write_local.rs`, `write_oracle.rs` |
| A session longer than its journal: the entry that does not fit is refused, so nothing a replay needs is overwritten | Refused | Unreleased (#112) | #113 (journal reclaim) | `oracle_write_local.rs` |
| Symlinks, hard links, mode and owner | Experimental | Unreleased (#43) | | `write_oracle.rs` |
| Extended attributes, any name length, at the slot the reference uses, on SipHash and crc32c inodes, colliding names in hash runs with whiteouts | Experimental | Unreleased (#43, #91, #119) | crc32c xattr slot observed (#106) | `oracle_xattr_slots.rs`, `oracle_write_local.rs`, `write_oracle.rs` |
| Colliding names (hash runs, whiteouts) | Experimental | Unreleased (#88) | | `oracle_write_local.rs`, `write_oracle.rs` |
| Names in crc32c directories | Experimental | Unreleased (#88) | | `oracle_write_local.rs`, `write_oracle.rs` |
| A new btree root, any btree id | Experimental: no write needs an id of 16 and above yet | Unreleased (#92) | #80 | `oracle_node_flags.rs` |
| A full btree node, rewritten or split; a root grows a level | Experimental | Unreleased (#45) | | `oracle_write_local.rs`, `write_oracle.rs` |
| Any write on 4096-byte blocks with 32 KiB nodes | Upcoming: nodes split, but no test writes on such an image yet | | #44 | |
| Compressed or non-crc32c data | Refused | | | |
| Per-inode options | Unobservable | | #81 | |
| Names in casefolded directories | Refused | Unreleased (#98) | | `oracle_casefold.rs` |
| Encrypted, multi-device, snapshotted filesystems | Refused | | | |

## Interfaces

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| C ABI: mount (path or fs_core device), volume info, stat, readdir, directory iterator, read, readlink, listxattr, getxattr | Supported | Unreleased (#34) | | `capi.rs`, `oracle_capi.rs` |
| `fs.bcachefs info`, `ls`, `cat`, `stat`, `tree` (`--features cli`) | Supported | Unreleased (#33) | | `tests/cli/` |
| `fs.bcachefs` write verbs: `put`, `mkdir`, `rm`, `rmdir`, `mv`, `ln`, `chmod`, `chown`, `setfattr`, `rmfattr` (experimental; the `cli` feature now includes `write`) | Experimental | Unreleased (#46) | | `tests/cli/test-write.sh`, `write_oracle.rs` |
