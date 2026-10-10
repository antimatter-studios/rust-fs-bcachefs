# Changelog

Notable changes to `rust-fs-bcachefs`, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This is a `0.x`
crate, so the **minor** is the compatibility boundary: a minor bump may break
API, a patch never does.

## [Unreleased]

### Changed

- **The writer's lookups read only the nodes on a key's path** (#108): an
  inode, a dirent slot, an inode's extents and a directory's entries go
  through `btree::Cursor`, with a journalled session's replay laid over the
  nodes, instead of a walk of the whole btree. `Writer::device` gives the
  device back.
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

- **The C ABI writes, behind the `write` feature** (#103):
  `fs_bcachefs_mount_rw` and entry points for create, mkdir, unlink, rmdir,
  rename, symlink, link, write_file, chmod, chown, setxattr and
  removexattr, by absolute path, each committed in place; failures come
  back as negative errnos (`include/fs_bcachefs.h`).
- **Per-file options are read** (#81): `Inode::option` gives a file's
  data checksum, compression, background compression and replicas as the
  reference tool names them, including options inherited from its
  directory; `Inode::options` keeps the stored fields. `Inode` gains a
  public field, so the minor version moves to 0.4.

- **Subvolumes and snapshots are read** (#12): `Filesystem::resolve` turns
  a path into a `Node` (an inode number and the snapshot it is read at),
  entering each subvolume on the way, and `inode_at`, `readdir_at`,
  `read_at`, `read_range_at` and `xattrs_at` read it as that snapshot sees
  it. The plain calls read the root subvolume; `lookup` refuses a path into
  another, whose inode numbers repeat in its snapshots.
- **Reflinked files are read** (#7): a `reflink_p` is followed into the
  reflink btree, laid out as the reference kernel module writes it. The
  kernel oracle clones a file with FICLONE and both read back as its mount
  reported.
- **Journal reclaim** (#113): when a journalled session's journal is full,
  everything it holds is written into the btree nodes in place, its
  accounting deltas summed, the superblock is marked clean at the newest
  sequence, and the session goes on in a fresh journal window, instead of
  refusing the write.
- **Buckets a writer frees are used again in the same session** (#132): a
  write that allocates first returns buckets emptied by earlier commits to
  the free pool, as the reference kernel module does, so a long session on
  a small image no longer runs out of space.
- **Data is written on every data checksum and on compressed
  filesystems** (#105): crc64 and xxhash in a crc64 entry, none as a bare
  pointer, as the formatter's fixtures store them; on a filesystem
  formatted with lz4 or zstd each piece is compressed (lz4 as a bare
  block, zstd length-prefixed) and stored so when that saves a block, or
  stored as it is and marked incompressible, as the reference stores data
  it cannot compress. On a gzip filesystem data is stored as
  incompressible: the reference mount crashed reading this writer's raw
  deflate (open question 20).
- **`Writer::write_at`, `append` and `truncate`** (#102): a write into
  part of a file, an append, and a truncation to any size. The file's
  contents are read (compressed and checksummed extents included, through
  the reader's own extent decoding), changed, and rewritten whole, laid
  out as `write_file` lays it out.
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

- **Nodes smaller than their bucket are written** (#109): a new node takes
  a bucket of its own and leaves the rest unused, as the accounting already
  recorded; the 4096-byte-block, 32 KiB-node geometry is now tested to
  split and grow.
- **A non-ASCII name in a casefolded directory is found in any case**
  (#111): names fold by Unicode full case folding after canonical
  decomposition, then NFD (`inode::casefold`), checked against every
  folded name the reference stored in the `casefold` set. The tables come
  from the `caseless` and `unicode-normalization` crates (MIT, MIT OR
  Apache-2.0).
- **Xattrs are written on crc32c-hashed inodes and when their slot is
  taken** (#106): the writer probes along the hash run and reuses
  whiteouts, as for dirents, and a removal inside a run leaves a
  `hash_whiteout`. The crc32c xattr slot is the dirents' crc32c hash over
  the namespace byte and the name, as the write study's `xcollide` images
  show.
- **rename replaces an existing file and moves a directory to another
  directory** (#104), as the write study shows the reference does: the
  replaced inode and its extents are deleted unless another name links it,
  the dirent keeps its slot, and the parents' `bi_nlink` follows the moved
  directory. A directory cannot move below itself; renaming over a
  directory is still refused.
- **A create no longer fails when the inode allocation cursor names a
  number in use** (#107): the writer takes the next free number above it
  and moves the cursor past it. How the reference picks a number then is
  unobserved: every fixture holds a single cursor.
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
- **Casefolded directories are read** (#54). The formatter makes them
  with `--casefold` although the reference mount refuses to set the
  option on a directory, and a `casefold` fixture shows their entries:
  the name as given and its folded form, with their lengths. Listings
  give the names as stored. A lookup finds an ASCII name in any case, as
  the reference mount does, and any other name as stored (#111). The
  writer refuses to change a casefolded directory.
- **An extent entry kind this reader cannot decode is refused by name**
  (crc128, stripe_ptr, flags, reconcile; #52), and a `bgcompress` fixture is
  built so a reconcile entry's layout can be observed.
- **Reflinked data is refused by name** (#7): a `reflink_p` in the extents
  btree is refused by the reader instead of reported as an unknown key type.
  The checker reports it as `reflink` instead of passing it unread, and the
  writer will not free it as if it were data. The fixture build tries every
  route to a clone through the reference mount (FICLONE, FICLONERANGE,
  `copy_file_range`, FIDEDUPERANGE). None makes a reflink. The reference
  tool's `kvdb` key editor is the route left for learning the layout.
- **crc128 and flags extent entries are read** (#52). A `crc128` fixture
  (`--encoded_extent_max=1M`) shows the entry the reference writes for
  extents over 512 sectors, and a `poison` image shows the flags entry its
  mount writes when a read finds bad data: a poisoned extent now fails to
  read with an I/O error, as the reference's does, and `fsck.bcachefs`
  passes the image as the reference checker does (`extent::poisoned`).
  The stripe pointer of an erasure-coded extent, seen on a three-device
  probe, is refused by name.
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
