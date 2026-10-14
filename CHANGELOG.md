# Changelog

Notable changes to `rust-fs-bcachefs`, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This is a `0.x`
crate, so the **minor** is the compatibility boundary: a minor bump may break
API, a patch never does.

Changes not yet released are fragments in [`changelog.d/`](changelog.d/), one per
pull request; a release gathers them into a section here.

## [Unreleased]

## [0.4.2] — 2026-10-14

### Added

- **Preallocated space reads as zeros (#158).** An extent whose pointer is
  marked unwritten, which the reference kernel module makes when a MiB is
  preallocated with `fallocate` in a `nocow` file, used to make the whole
  file unreadable; it now reads as zeros over its range, as the reference
  reads it, and the checker accepts it. The flag was observed rather than
  guessed: the kernel oracle builds a fifth image for it, and
  `tests/oracle_unwritten.rs` reads both a `nocow` and an ordinary
  preallocated file back as their 4 KiB in a MiB of zeros. A pointer with
  any other flag (S1's cached pointers, never observed) is still refused.

### Fixed

- **One damaged alloc node no longer makes every file unreadable.** Every
  data read checked its pointer's generation against the bucket's alloc
  key, so a torn alloc node failed every file though the data and its
  checksums were intact. When the alloc btree cannot be read, the
  generation now comes from the bucket_gens btree, and when that has none
  either, the extent's own data checksum stands in; an extent with no
  checksum is still refused (#167).

- **The checker judges allocation information only on a filesystem shut
  down cleanly (#163).** After an unclean shutdown the allocation btrees
  can lag the rest of the filesystem, as the reference leaves them in its
  own crash images (a bucket's generation in bucket_gens one ahead of its
  alloc key), and the reference rebuilds them in recovery. The checks of
  bucket_gens, backpointers, data types, the fragmentation lru,
  need_discard and accounting reported such an image as damaged; they are
  now made only when the filesystem was shut down cleanly.

- **The checker reports more allocation damage (#163): a data bucket of
  the wrong type, a fragmentation lru, need_discard btree or device
  accounting out of step with the buckets, and a missing backpointer of a
  piece of a checksummed region.** Each bucket holding a written data extent, or an indirect
  extent's data, must be a user bucket in its alloc key; one that is not,
  or that has no alloc key, is reported as `alloc_key_data_type_wrong`. On
  a single-device filesystem each partly filled user or btree bucket must
  have its fragmentation lru entry, as the reference checker requires (one
  without is `missing_fragmentation_lru_entry`, an entry no bucket's fill
  gives is `incorrect_lru_entry`), and each bucket waiting for discard its
  need_discard key (`missing_need_discard_entry`, and
  `incorrect_need_discard_entry` for a key no such bucket gives), and the
  device's accounting of each data type must be what its alloc keys add
  up to (`accounting_mismatch`, the reference checker's words). A piece
  of an uncompressed checksummed region, which the reference leaves where
  it overwrote part of a file, must have its backpointer at its region's
  sector plus its offset, and is reported as `missing_backpointer` without
  it.

- **The checker reads the allocation btrees' bucket generations (#163).**
  A bucket whose generation in `bucket_gens` is not the one its alloc key
  holds is reported as `incorrect_gen_in_bucket_gens`, as the reference's
  checker reports "incorrect gen in bucket_gens". The checker judged no
  allocation btree before.

- **The checker reports a data extent with no backpointer (#163).** Each
  data extent written whole, not unwritten and not a piece of a checksummed
  region, must have its backpointer at its device sector, as the reference
  checker requires ("missing backpointer"); one without is reported as
  `missing_backpointer`, naming the extent.

- **The checker finds xattr, back-reference and options-flag damage
  (#163).** `check::check` and `fsck.bcachefs` now report an xattr whose
  inode is gone (`key_in_missing_inode`), an xattr its name's hash slot
  cannot reach (`hash_table_key_wrong_offset`), an inode whose
  back-reference names no entry or another inode's
  (`inode_points_to_missing_dirent`, `inode_points_to_wrong_dirent`), and an
  inode whose has-options flag disagrees with its option fields
  (`inode_has_inode_opts_flag_wrong`). The reference checker found each of
  these on images where this one reported nothing.

- **A crafted reservation, whiteout or extent size no longer aborts the
  process.** A reservation or whiteout read as zeros through a buffer as
  large as its key, and an extent with no checksum entry was read into a
  buffer as large as the extent, both before anything bounded them; a key
  claiming up to 2 TiB aborted the process. Reservations and whiteouts now
  zero only the bytes a read asked for, and an extent whose stored sectors
  run past the end of the device is refused as corrupt (#151).

- **A create, link, rename or removal reads only its name's run of dirent
  hash slots.** Finding a name's slot read every dirents key of the directory
  from the name's hash on, so in a large directory one create could read
  most of its dirents nodes; it now stops at the first offset its run
  skips (#191).

- **A write or truncate past the device's size no longer aborts the
  process.** The writer rewrites a file whole and held the new size in
  memory before checking it, so a truncate to 1 TiB, or a one-byte write
  at 1 TiB, asked for a terabyte and aborted the caller, C ABI hosts
  included. A new size larger than the device is now refused before
  anything is allocated (#156).

- **A btree node is checked against the pointer that leads to it.** The
  reader believed what a node's header said about itself, so a node naming
  another btree or another level, or starting or ending somewhere its
  pointer does not, fed its keys into the wrong place. Such a node is now
  refused, as is a key format whose fields unpack past their width and a
  bset whose keys are out of order, both of which wrapped or re-sorted keys
  silently (#162).

- **Unlinking a reflinked file reads its run of indirect extents, not the
  rest of the reflink btree (#193).** Dropping a `reflink_p`'s references
  collected every reflink key from the pointer's start to the end of the
  btree before it looked at any of them, so its cost grew with the reflink
  btree rather than with the file. It now reads through a cursor and stops
  at the run's end or at its first gap.

- **Removing a directory reads up to its first entry, not all of them.**
  The check that a directory is empty read every dirents key it held, so
  refusing to remove a large one read its every leaf; it now stops at the
  first live entry (#193).

- **A superblock root that will not read is replaced by the journal's
  (#166).** The journal records every btree root again, and on every clean
  fixture it records the same roots as the superblock's clean field. The
  reference opened images whose clean-field root had been damaged, and so
  does this reader now: when a root's key does not decode, its node does not
  read, or the node belongs to another btree or level, `Filesystem::open`
  and the checker read the filesystem through its journal, as after an
  unclean shutdown, instead of refusing it or failing that btree.

- **A damaged journal entry outside the replay window no longer makes the
  filesystem unreadable.** The journal is a ring, and entries older than
  the window are stale writes recovery never reads, but the reader parsed
  every entry and refused the filesystem over one damaged anywhere. Damage
  older than the window, or newer than its last flush, is now passed over;
  damage inside the window is still refused (#165).

- **The CLI tarball and the static library carry their crates' licence
  notices.** Both link nineteen crates under MIT, Apache-2.0, Zlib and
  0BSD, and carried only this repository's `LICENSE`. They now ship
  `THIRD-PARTY-LICENSES.md` beside it, generated from `cargo metadata` by
  `scripts/third-party-notices.py`, and `chore lint` fails when a shipped
  crate has no notice there (#155).

- **Truncating a file up leaves the new range a hole (#156).** A truncate
  up rewrote the file with the added range as allocated zeros, so an
  empty file truncated to 1 MiB took a MiB of buckets, and a sparse file
  lost its holes. Only the file's last block is rewritten now, zero-filled
  past the old end, and the size set; the added range has no extent and
  reads as zeros. A write past the end still writes the gap as zeros.

- **A file held by pieces of checksummed regions can be rewritten
  (#169).** Where the reference overwrites part of a file, it leaves
  extents whose checksum entry covers more than they do, and the writer
  refused to free them ("freeing part of a checksummed extent is not
  implemented"), so any write, truncate or unlink of such a file failed.
  A piece of an uncompressed region is now freed as the reference counts
  it: its live sectors from its bucket, and its backpointer at the
  region's sector plus its offset. A piece of a compressed region, which
  no image has shown, is still refused.

- **A write to a file with preallocated space keeps that space reading as
  zeros (#207).** The writer rewrites a file whole from what it reads of
  it, and it read an unwritten extent's device sectors as the file's data,
  so any `write_at`, `append` or `truncate` on such a file stored whatever
  the device held there, a deleted file's bytes on a reused bucket. Those
  ranges are now zeros, as the reader reads them.

- **A reflinked file can be written (#169).** `write_at`, `append` and
  `truncate` refused a file whose data is shared through a `reflink_p`
  ("extent key type 15 cannot be rewritten"), so a cloned file could not be
  modified at all. The rewrite now reads the shared data and copies it into
  extents of the file's own, dropping one reference to each indirect
  extent; the last reference frees it, as an unlink does.

- **Setting an xattr reads its run of slots, not the rest of the table.**
  The probe for an xattr's slot read every xattr key of the inode from the
  name's hash onwards, so on an inode with many xattrs one set read most of
  its table; it now stops at the first slot its run skips (#193).

## [0.4.1] — 2026-10-12

### Fixed

- **A file whose inode claims an impossible size is refused instead of
  taking down the process.** Reading a file sized its buffer from the
  inode's size before reading anything: past `isize::MAX` that panicked, and
  past what memory holds the allocation failed and Rust aborted, which no
  caller and no C ABI guard can catch. A whole-file read now refuses a file
  that claims more than its device holds (it can only be mostly holes; read
  it in ranges), the buffer is reserved fallibly, and `fs_bcachefs_readlink`
  refuses a symlink target over 4096 bytes; the writer's partial writes are
  bounded the same way (#149).

- **A write refused for space leaves the file as it was.** A commit writes
  each btree's change to disk as it goes, and data could take a filesystem's
  last free buckets: a later node split then found none, and a write that
  returned an error had already changed the file (on `block4k`, the last
  byte of a 10-byte file). Data now leaves 32 buckets free for btree nodes,
  so a write that would need them is refused before anything is written
  (#168).

## [0.4.0] — 2026-10-12

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

### Fixed

- **The C ABI's errnos are the platform's numbers** (#188): the writes
  returned Linux's numbers everywhere, so on macOS a non-empty `rmdir` came
  back as -39 rather than -ENOTEMPTY (66), and ENOTSUP as -95 rather than
  45.

- **The checker reports an extent far past the end of its file.** It
  compared an extent's start with its file's size in bytes, and a start 2^55
  sectors in overflowed: a panic with overflow checks on, and in a release
  build a wrap to 0 that judged the extent inside the file. It compares in
  sectors now (#150).

- **Removing a file removes its extended attributes.** Unlinking a file's
  last name, removing a directory, or renaming over a file deleted the inode
  and freed its data but left its xattr keys behind, which the reference
  checker reports as `key_in_missing_inode`. They go in the same transaction
  as the inode now (#161).

- **The writer honours per-file options** (#142): data is checksummed and
  compressed as the file's `data_checksum` and `compression` say, the
  filesystem's where the file has none, and a new inode takes its
  directory's options, stored but not marked set, with the has-options
  flag, as the reference kernel module's own file there does.
- **Data on a gzip filesystem is written gzip-compressed** (#141): as a
  raw deflate stream, the framing the reference kernel module uses. The
  kernel oracle has this crate's tools write a gzip file, and the module
  reads it back byte for byte; it was stored as incompressible since open
  question 20.
- **The writer frees reflinked data** (#140): unlinking or truncating a
  file drops one reference from each indirect extent its `reflink_p`
  covers, and the last reference deletes the indirect extent and frees its
  data, counted at inode 0 as the reference counts shared data. A pointer
  to nothing is refused as corrupt.
- **The checker judges snapshots and follows reflinks** (#139): entries,
  inodes and link counts are checked as each subvolume sees them through
  its snapshot's chain, an entry naming a subvolume is checked against the
  subvolumes btree, and every `reflink_p` against the reflink btree: its
  range is covered, each indirect extent's refcount matches the pointers
  into it, and its data is checked. Every image the reference kernel module
  made is clean to it, as it is to the reference checker.
- **The writer refuses a filesystem whose root subvolume is snapshotted**
  (#138): its keys are at snapshot u32::MAX, which such a root shares with
  its snapshot, so a write there changed the snapshot too. The kernel
  oracle makes such a filesystem (`kernel-rootsnap.img`).
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
