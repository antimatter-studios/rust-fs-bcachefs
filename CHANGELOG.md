# Changelog

Notable changes to `rust-fs-bcachefs`, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This is a `0.x`
crate, so the **minor** is the compatibility boundary: a minor bump may break
API, a patch never does.

## [Unreleased]

### Changed

- **Lookups, listings and reads go through a lazy btree cursor**
  (`btree::Cursor`): nothing but the superblock is loaded at open, and a
  lookup reads only the nodes on its path; a name is found at its hash.
  `Filesystem::inode` and `Filesystem::readdir` return owned values, and
  `Inode` carries `hash_seed`: the minor version moves to 0.2.
- **The crate describes itself as read-only by default with an optional
  `write` feature**, rather than read-only outright.
- **The test VM deletes the reference tools' source tree** as soon as their
  binary is installed, so nothing in the guest can open it by accident.

### Added

- **A read-only spike of a clean-room bcachefs reader.** The superblock is
  parsed and checksummed; btree nodes are read, checksummed and walked;
  inodes, directory entries and extents are decoded; files read back byte
  for byte, with crc32c, crc64 and xxhash data checksums and lz4, zstd and
  gzip compression. Every structure is compared with the reference tools'
  view of eight fixture sets.
- **A C ABI** (`fs_bcachefs_mount`, `_stat`, `_readdir`, `_read_file`,
  `_umount`) and the `fs.bcachefs` tool (`info`, `ls`, `cat`).
- **Fuzzing the way the family does it.** Seven `cargo-fuzz` targets
  (superblock, btree node, journal entry, key values, inode_v3 with a
  decode-encode round trip, extent entries, the LZ4 block decoder) share
  their drivers with the gate, `tests/fuzz_decoders.rs`, which replays and
  mutates the corpus on every pull request with hang detection and a
  case floor. Checksums are re-stamped by the harness so the decoders
  behind them are reached: the `fuzzing` cargo feature that switched
  verification off in the library is gone, which removes a cargo feature
  and so moves the minor version to 0.3. `scripts/make-fuzz-corpus.sh`
  (`chore fuzz:corpus`) cuts the seeds from the fixtures and keeps
  committed reproducers; `chore test:scripts` tests both scripts.
- **Fixtures made by the reference formatter in the Linux test VM**, each with
  a JSON record of what the reference tools say is in it.
- **docs/clean-room.md**, the provenance of every fact about the format,
  the licence position on the specification it was learned from, and the
  open questions.
- **A licence gate**: `chore lint` runs `cargo deny check licenses` against
  `deny.toml`, which allows permissive licences only; CI installs cargo-deny
  for it.
- **Journal replay, in memory**: a filesystem that was not cleanly unmounted
  is read through its journal (`journal::read_entries`, `journal::replay`,
  `btree::walk_replayed`), as the reference sees it after its own replay,
  without writing to the device.
- **Extended attributes** (`Filesystem::xattrs`, `xattr::Xattr`): the
  user and trusted namespaces, read through a journal replay too.
- **`fs.bcachefs stat` and `tree`**: one path's inode (number, type, mode,
  owner, links, size, sectors, times) and every path under a directory.
- **C ABI**: `fs_bcachefs_last_error`, `_mount_with_fs_core_device`,
  `_get_volume_info`, `_stat_ino`, a directory iterator (`_dir_open`,
  `_dir_next`, `_dir_close`), `_readlink`, `_listxattr` and `_getxattr`.
- **`fsck.bcachefs`, check-only** (`check::check`): btree nodes, key order,
  directory entries, link counts, extents and data checksums; fsck(8) exit
  status. Never writes.
- **Writing, behind the `write` feature (off by default)**:
  `write::Writer` appends bsets to btree nodes and carries their length to
  the superblock, and `Writer::create_file` creates a small (inline) file in
  an existing directory; `mkdir`, `unlink`, `rmdir`, `rename` and
  `write_file` (whole inline contents) follow, and files larger than
  inline data are written into allocated buckets with every alloc,
  freespace, backpointer, lru and accounting key the reference writes;
  unlinking or rewriting them frees their buckets for discard; and
  `Writer::journal_commits` commits each operation as a journal entry the
  reference replays, so an interrupted write is recovered whole or not at
  all; `symlink`, `link`, `set_attributes`, `set_xattr` and
  `remove_xattr` round it out. Full btree nodes are rewritten or split into
  fresh buckets. `Writer::open_journalled` continues a journal left for
  replay, and `fs.bcachefs` carries the write verbs (`put`, `mkdir`, `rm`,
  `rmdir`, `mv`, `ln`, `chmod`, `chown`, `setfattr`, `rmfattr`), marked
  experimental, with `--journal`. Every image it writes in the tests passes the
  reference checker and reads back through the reference implementation.
- **`Filesystem::read_range`** (#57): a window of a file, reading only the
  extents that cover it; `fs_bcachefs_read_file` uses it, so a C consumer
  reading a file in pieces no longer decodes the whole file per piece.
- **Encoders**: `inode::InodeV3Raw`, `inode::varint_encode`,
  `Dirent::encode_value`, `inode::dirent_hash` and `siphash::siphash24`.
- **Inline data**: small files a mounted filesystem stores inside the
  extents btree read back byte for byte.
- **An aged fixture**, mounted and aged by the reference implementation in
  the test VM, with hard links, renames, deletions, overwrites, sparse and
  fragmented files; every file and listing is compared with the mount's view.

- **Encrypted and multi-device filesystems are refused by name**
  (`Superblock::is_encrypted`), and `fs.bcachefs info` reports `encrypted`
  and `clean`.

### Fixed

- **A journalled session that fills its journal is refused instead of
  overwriting its own entries** (#101): nothing is reclaimed, so every entry
  since the session began is one a replay needs. The ring used to wrap onto
  the oldest of them, so a long session, or a chain of `--journal` runs,
  wrote over transactions a replay still needed. Now the entry that does not
  fit is refused ("the journal is full"); journal reclaim is #113.
- **A transient HTTP 5xx from the chore release download no longer fails a CI
  job.** `scripts/ci-install-chore.sh` retries both downloads up to five
  times on any error; the checksum check still guards what was fetched.
- **`fs_bcachefs_stat` reports the link count as a mount does**, not the
  stored count.

- **An uncleanly unmounted filesystem is no longer read from its stale
  superblock roots**: `btree::walk` refuses it, and `Filesystem::open`
  replays its journal.
- **Key positions print as the reference lister prints them** (`POS_MIN`,
  `SPOS_MAX`, `U64_MAX`).
- **Link counts** are reported as a mount reports them (`Inode::link_count`).
- **The error for a superblock without a `clean` field** no longer says
  journal replay is not implemented; it points at the journal.
- **A filesystem with snapshots is refused, not misread** (#53): every key
  must carry the root inode's snapshot, and `fsck.bcachefs` reports
  `snapshots` when one does not.
- **Overlapping extents are refused by the reader and reported by the
  checker** (`extent_overlap`, #60) instead of the later key silently
  overwriting the newer one's bytes.
- **An unknown checksum type never verifies** (#48): `csum::verify` returns
  an error for a type it cannot compute instead of passing it.
- **The fixture build probes the reference mount for a subvolume, a
  snapshot, a casefolded directory and a reflink** on a scratch image and
  records the answers and the lister's view (`probe.*`); `tests/oracle_probes.rs`
  asserts what the reader does with the image either way, so the fixtures
  behind #12, #54 and #7 turn a test red the first time the reference
  implementation honours one of them, and `tests/oracle_bgcompress.rs` does
  the same for an extent entry kind this reader cannot decode (#52).
- **An extent entry kind this reader cannot decode is refused by name**
  (crc128, stripe_ptr, flags, reconcile; #52), and a `bgcompress` fixture is
  built so a reconcile entry's layout can be observed.
- **Reflinked data is refused by name** (#7): a `reflink_p` in the extents
  btree is refused by the reader instead of reported as an unknown key type.
  The checker reports it as `reflink` instead of passing it unread, and the
  writer will not free it as if it were data. The fixture build tries every
  route to a clone through the reference mount (FICLONE, FICLONERANGE,
  `copy_file_range`, FIDEDUPERANGE). None makes a reflink, so the layout
  stays unobservable.
- **The writer refuses a filesystem whose time precision is not
  nanoseconds** (#58), since the times it stamps would be in the wrong unit;
  the guest test that could pass by refusing a full node is named for both
  of its outcomes, and the unconditional create-and-read-back stays on the
  write-study base.
- **Directories hashed with crc32c or crc64 are scanned, not mis-hashed**
  (#51): `Inode::hash_type` reads the inode's string hash type, the reader
  falls back to a scan for any type but SipHash, and the writer refuses to
  place a name in such a directory. A `strhash` fixture (`--str_hash=crc32c`)
  exercises the scan.
- **A cursor past the last key stands before nothing** (#56): the end of a
  btree is the root key's position, not an assumed all-ones one, and a seek
  beyond it is not an error.
- **Extent pointers are judged before they are read** (#59): the bucket's
  generation from its `alloc_v4` key must match (a stale pointer is
  refused), the device must be this one, and a pointer with any flag set
  (cached, unwritten) is refused; the checker reports `extent_pointer`.
- **An `error` extent reads as an I/O error naming the lost range** and the
  checker reports `data_lost` (#61); an `extent_whiteout` reads as a hole;
  an `inode` or `inode_v2` key is refused as an older encoding instead of
  being reported as a missing inode (#55).
- **Bsets from blacklisted journal sequences are ignored** (#50): the
  superblock's `journal_seq_blacklist` (`Superblock::journal_seq_blacklist`,
  end exclusive) is applied to every node read, and a journal replay window
  carrying a blacklist entry of its own is refused.
- **Every superblock copy is read and the highest `seq` wins** (#49), with
  the layout at sector 7 as the fallback when the primary is gone;
  `fsck.bcachefs` reports a copy that does not read (`superblock_copy`).
