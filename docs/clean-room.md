# Clean-room record

This crate reads bcachefs. bcachefs's implementation (the kernel's
`fs/bcachefs` and the userspace tools) is GPL-2.0; this crate is MIT. To keep
it that way, the on-disk format is learned only from:

1. **prose documentation** of the format, and
2. **black-box observation**: hexdumps of images the reference tools made, and
   the reference tools' own printed output about those images.

No GPL source was read, in any form (repository, crate, search snippet), by
anyone writing this crate. The reference tools are built and run only inside
the disposable Linux test VM (`scripts/vm-setup.sh`), are never linked or
copied here, and are named by role: the **reference formatter**
(`format`), the **reference superblock printer** (`show-super`), the
**reference lister** (`list`), the **reference checker** (`fsck`), and the
**reference implementation**: the tools' userspace copy of the filesystem
mounted through FUSE (`fusemount`), which ages the `aged` fixture inside the
VM by ordinary file operations.

Every source used is listed below with what it told us. A fact that could not
be learned this way is an **open question**, not a guess.

## Sources

| # | Source | Kind | What it told us |
|---|---|---|---|
| S1 | "bcachefs: Principles of Operation", https://bcachefs.org/bcachefs-principles-of-operation.pdf (fetched 2026-10-06) | prose documentation | Superblock at sector 8 (4 KiB), layout copy at 3584 bytes; superblock carries UUIDs, label (32 bytes), block size, btree node size, device count, version and minimum version, seq, typed variable-length fields (journal, members_v2, clean, ...); 28 btrees by name; `struct bpos {u64 inode; u64 offset; u32 snapshot}` and `struct bkey {u8 u64s; u8 format; u8 type; u8 pad; bversion; u32 size; bpos p}` as documented C declarations; packed keys use a per-node `bkey_format` with a base offset and bit width for six fields (inode, offset, snapshot, size, version_hi, version_lo) and keep a 3-byte header; extent key positions are the END of the extent; btree node = header (checksum, magic, seq, flags with btree id and level, min/max key, format, first bset) followed by `btree_node_entry` bsets; extent values are a list of entries whose type is the position of the first set bit of the first word; crc32/crc64/crc128 entry sizes (8/16/24 bytes); pointer has a device, a 44-bit sector offset and a generation; list of key types and metadata versions in order. |
| S2 | bcachefs-tools `INSTALL.md` at v1.39.7 (build instructions only) | prose documentation | The reference tools' build dependencies, minimum Rust and the `BCACHEFS_FUSE=1` switch for their FUSE mount, used only to build the oracle inside the VM. Contains no format information. |
| S3 | Reference tools v1.39.7, run in the test VM | black-box oracle | `show-super`, `list`, `fsck` output for each fixture; recorded in `.vm-share/fixtures/*.json` and `*.txt` beside the images. |
| S4 | Hexdumps of fixture images, and candidate computations over their bytes (checksum conventions) | black-box observation | See the per-structure notes below. |
| S5 | The LZ4 block format description, https://github.com/lz4/lz4/blob/dev/doc/lz4_Block_format.md (BSD-2-Clause documentation of a public format) | prose documentation | Token, literal-length and match-length encoding, little-endian 16-bit offsets, overlapping matches: the decoder in `src/compress.rs` is written from it. |
| S6 | Published check values: CRC-32C of "123456789" (0xe3069283), CRC-64/WE of "123456789" (0x62ec59e3f1a4f00a), XXH64 of the empty input with seed 0 (0xef46db3751d8e999) | public reference values | Unit-test anchors for the checksum implementations. |
| S8 | The reference implementation mounted through FUSE in the test VM (tools v1.39.7 built with `BCACHEFS_FUSE=1`, per S2), driven by ordinary file operations (`scripts/guest-age.py`) | black-box oracle | The `aged` and `aged-unclean` fixtures: what a running filesystem writes (inline data, narrow key formats, nodes of many bsets, link counts, an unclean shutdown), with the inode numbers and link counts the mount reported. |
| S7 | `bcachefs-tools` GitHub API metadata (tag list, `Cargo.toml` `rust-version` field only) | metadata | Which release to pin (v1.39.7) and the minimum Rust to build it with in the VM. No source file was opened. |

## Per-structure notes

Each entry: the fact, and which source it came from. "Checked" means an
oracle test in `tests/oracle_*.rs` compares it against the reference tools for
all eight fixture sets.

### Superblock (`src/superblock.rs`) -- documented location, inferred layout

- At byte 4096; the layout copy at 3584 (S1). Little-endian throughout (S4).
- Offsets (S4, checked against the superblock printer): csum[16] @0x00;
  version u16 @0x10 and oldest-version u16 @0x12, each `major << 10 | minor`
  (1.39 = 0x427; the version names are S1's version history); magic[16]
  @0x18; internal UUID @0x28; external UUID @0x38; label[32] @0x48; offset
  u64 @0x68; seq u64 @0x70; block_size u16 (sectors) @0x78; dev_idx u8
  @0x7a; nr_devices u8 @0x7b; u64s u32 @0x7c (length of the fields in
  u64s); time base @0x80..0x90; flags[8] u64 @0x90; features[2] @0xd0;
  compat[2] @0xe0; embedded layout @0xf0; fields from 0x2f0.
- Layout: magic[16], type u8, sb_max_size_bits u8 (sectors, log2), nr u8,
  5 pad, then 61 u64 sector offsets (S4; "up to 61 backup locations" is S1).
- Fields: u32 u64s, u32 type, body. Type numbers follow S1's list order,
  confirmed by the printer's "Sections" line.
- Checksum: covers byte 16 to the end of the fields. Type from flags[0] bits
  2..5 (inferred): 1 = standard crc32c, 2 = CRC-64/WE (ECMA-182 polynomial,
  MSB first, init and xorout all ones), 7 = XXH64 seed 0 -- each found by
  computing candidates over a fixture (S4, S6) and checked.
- Options (inferred, checked): btree_node_size = flags[0] bits 12..27
  (sectors); metadata/data checksum option = flags[0] bits 40..43 / 44..47
  (none 0, crc32c 1, crc64 2, xxhash 3); compression option = flags[1] bits
  4..7 (none 0, lz4 1, gzip 2, zstd 3).
- Feature names: the printer lists the names of set bits in ascending order;
  pairing them gave lz4 0, gzip 1, zstd 2, new_siphash 7, ... (checked).
- members_v2: u16 member size, 6 pad, then per member uuid, nbuckets u64,
  first_bucket u16, bucket_size u16, 4 unread bytes, last_mount u64 (S4,
  checked).
- clean: flags u32, two u16 clocks, journal_seq u64, then journal entries
  (u16 u64s, u8 btree_id, u8 level, u8 type, 3 pad); type 1 = btree_root,
  matching S1's journal entry list order (S4, checked by walking the roots).

### Keys (`src/bkey.rs`) -- documented members, inferred byte order

- `bpos` and `bkey` members are S1's documented declarations. On disk the
  bpos is snapshot u32, offset u64, inode u64 (reverse of the documented
  order); an unpacked key is u64s, format (1 = unpacked), type, pad,
  version (12 bytes), size u32, bpos (S4, checked).
- Packed keys (format 0): the node's bkey_format is key_u64s u8, nr_fields
  u8, bits[6] u8, field_offset[6] u64 (S1 names the fields; S4 the layout,
  confirmed by the lister's `formats` mode). Fields are taken from the most
  significant bit of the key's words downward (S4; checked on byte-aligned
  formats only -- see open questions).
- Key type numbers follow S1's list order: extent 6, dirent 10,
  btree_ptr_v2 18, inode_v3 29 (checked).

### Btree nodes (`src/btree.rs`) -- documented structure, inferred layout

- Header: csum[16], magic u64 @16, flags u64 @24, min_key @32, max_key @52,
  8 unread bytes @72, bkey_format @80, first bset @136. Bset: seq u64,
  journal_seq u64, flags u32 (low 4 bits: checksum type), version u16, u64s
  u16, keys. Later bsets are `btree_node_entry`s: csum[16] then a bset,
  starting on block boundaries, with the node's seq (S1 structure; S4
  layout, checked by key-for-key agreement with the lister).
- Node magic = 0x90135c78b99e07f5 XOR the first 8 bytes of the internal UUID
  (S4: the XOR is constant across every fixture).
- Checksum: from byte 16 of the record to the end of its keys; type 1
  standard crc32c, 2 CRC-64/WE, 7 XXH64 (S4, checked).
- btree_ptr_v2 value: mem_ptr u64 (unread), seq u64, sectors_written u16,
  flags u16, min_key, then extent pointers (S1 names; S4 layout).
- Btree ids follow S1's list order: extents 0, inodes 1, dirents 2
  (checked).

### Inodes and dirents (`src/inode.rs`) -- documented fields, inferred encoding

- Inodes are keyed by number in the bpos OFFSET field (S3: the lister
  prints `0:4096:U32_MAX`). Root is 4096 (S3).
- inode_v3: journal_seq u64, hash_seed u64, flags u64 (bits 24..31 number of
  varint fields, bits 20..23 hash type, bits 36..51 mode), then sectors u64,
  size u64, version u64, then varints in the lister's field order with each
  time taking two varints (S3 + S4, checked for mode, size, sectors, uid,
  gid, nlink, atime, ctime, mtime on every inode).
- Varint: length = trailing one bits of the first byte + 1; 9 bytes = next 8
  verbatim; else little-endian bytes >> length (S4, checked).
- Dirent value: inode u64, DT_* type u8, NUL-padded name (S4, checked).

### Extents and data (`src/extent.rs`, `src/fs.rs`, `src/compress.rs`)

- Entry types by lowest set bit (S1); bit layouts in `src/extent.rs` found by
  hexdump against the lister's printed fields (S3, S4), checked by reading
  every file of every fixture byte for byte.
- Data checksums: type 5 = crc32c from zero, not inverted; 6 = CRC-64/WE
  from zero, not inverted; 7 = XXH64 seed 0 (S4, checked).
- Compression numbering in crc entries: gzip 2, lz4 3, zstd 4,
  incompressible 5 (S3 + S4, checked). lz4 = bare LZ4 block; zstd = u32
  length + standard frame; gzip = raw deflate (S4, checked).
- Small files and symlink targets were stored as ordinary extents, not
  inline data, by this formatter (S3). A mounted filesystem stores small
  files as `inline_data` keys (type 17, S1's list order) in the extents
  btree: the value is the file's bytes zero-padded to a whole u64 (the
  lister's `datalen` is the value length), and the key covers `size`
  sectors ending at its position like any extent (S3, S4, S8; checked by
  reading every file of `aged`).

### What an aged filesystem added (`aged`, S8)

- Feature bits 5 = `journal_seq_blacklist_v3` and 8 = `inline_data`, by the
  same pairing as the other names (S3, checked).
- Clean shutdown: superblock flags[0] bit 1. The printer's `Clean: 1` and
  `Clean: 0` images (aged, aged-unclean) differ in flags[0] in that bit
  alone, and it is set on every formatter-made image (S3, S4, inferred).
  The reader refuses an image without it: its roots are stale until the
  journal is replayed (open question 4).
- Link counts: a file stores one less than its link count (three names,
  stored 2); a directory stores its number of subdirectories, and the mount
  reports that plus 2 (S3 against S8's `st_nlink` for every inode, checked).
- Leaf nodes with packed formats narrower than 64/64/32 and non-zero field
  offsets (`fields 16:2147485236, 8:1, 32:0, 8:0`; the lister's `formats`
  mode, recorded as `aged.<btree>.formats.txt`), and nodes of up to 78
  bsets with deletions among them: every key the lister prints is read,
  in order, with the same type, position and size (checked).

### Journal and replay (`src/journal.rs`) -- documented behaviour, inferred layout

- Documented (S1 9.7, 11.2): a ring of buckets of `jset`s with increasing
  sequence numbers; sub-entries typed in the listed order (btree_keys 0,
  btree_root 1, ..., log 9, overwrite 10, ...); btree roots recorded on
  every write; recovery replays from the newest flush entry's `last_seq`
  to that entry; entries after the last flush are not replayed; btree
  data from never-committed sequence numbers is ignored.
- `journal_v2` field (type 9): `(first bucket u64, count u64)` pairs; the
  journal's buckets are at `bucket * bucket_size` sectors (S4: every entry
  the reference lists is found at the sector it names, checked).
- jset: csum[16], magic u64 @16 (= 0x245235c1a3625032 XOR the internal
  UUID's first 8 bytes; constant across every entry, inferred), seq u64
  @24, version u32 @32, flags u32 @36 (low 4 bits checksum type; bit 5 set
  on the entries the reference calls `flush 0`), u64s u32 @40, last_seq
  u64 @48, sub-entries @56; the checksum covers bytes 16 to the end of the
  sub-entries; each entry is padded to the block size (S4, every header
  field checked against `list_journal -H` for all 201 entries).
- Sub-entry header: the same 8 bytes as the clean field's entries; keys
  are unpacked bkeys (S4; every `btree_keys` key of the replay window
  checked against `list_journal -d` by btree, position and size).
- Btree ids beyond dirents follow S1's list order; those the journals
  touch (alloc 4, lru 10, freespace 11, need_discard 12, backpointers 13,
  deleted_inodes 16, logged_ops 17, accounting 20) are checked.
- Replay in memory: the roots are the newest flush entry's `btree_root`
  entries, each with its level; bsets whose journal sequence (bset bytes
  8..16) is above that entry are ignored; the replayed keys, leaf and
  interior (the journal carries level-1 pointer updates when nodes split),
  replace the keys at their positions in the nodes they fall within, a
  deleted key or whiteout removing one. Checked: the uncleanly unmounted
  `aged-unclean` reads exactly as the reference sees it after its own
  replay, every path, listing and file byte.
### Refused filesystems

- Encryption: the formatter's `--encrypted --no_passphrase` adds a `crypt`
  field (type 2, S1's field order; the printer's `Sections` line) and the
  btree nodes no longer parse (S3, S4). The reader refuses any image with
  that field. Key derivation and nonces remain open question 6.
- Multiple devices: each member's superblock carries `nr_devices` 2 (the
  printer's `Devices`, checked); the reader refuses it.
### What each write changes (the write study, `fixtures/write-study`, S8)

Each pair is the same settled base image before and after one operation
through the reference implementation's mount; every btree of both is dumped
by the reference lister (scripts/guest-write-study.sh). Keys that changed,
by operation (positions `inode:offset:snapshot`):

- **create a small file** (6 bytes): extents + `inline_data` at
  `ino:1`; inodes + the new `inode_v3` (`bi_dir` = parent, `bi_dir_offset`
  = the dirent's offset, a fresh `hash_seed`) and the parent's
  `bi_ctime`/`bi_mtime`/`journal_seq` updated; dirents + `dirent` at
  `parent:hash(name)`; accounting `nr_inodes` +1 and the per-btree
  counters of extents, inodes and dirents; logged_ops' `inode_alloc_cursor`
  `consumed` +1 and `idx` = the next inode number.
- **create an empty file**: as above without the extents key.
- **mkdir**: as create-empty, the inode 19 u64s (a directory carries one
  more field).
- **unlink**: the dirent, the inode and its inline data are deleted;
  accounting down; here the filesystem also allocated a node for the
  deleted_inodes btree (alloc, freespace, backpointers and the btree
  accounting changed with it).
- **rename** within a directory: one dirent deleted, one added at the new
  name's hash; the dirents counter's bytes change.
- **truncate to 0 / overwrite** of an inline file: the inline_data key is
  removed / replaced; the extents counter changes.
- **create a 300000-byte file**: ten `extent` keys of up to 64 sectors,
  alloc_v4 keys for the buckets used, freespace ranges shrunk, an lru
  entry, backpointers, accounting.
- The per-btree accounting key (`snapshot id=... btree=NAME a b 0`) holds
  the number of leaf keys and their total size in bytes (u64s * 8): every
  pair adds or removes exactly the keys it changed (inferred, every pair).

### Extended attributes (`src/xattr.rs`, S8)

- The xattrs btree (id 3, S1's order) holds one `xattr` key (type 11) per
  attribute at `inode:hash:snapshot`. Value: namespace u8, name length u8,
  value length le16, the name without its namespace prefix, the value,
  zero padding (S3 + S4: hexdumps against the lister's `name:value` and
  the mount's listing; checked for every path of aged and aged-unclean).
- Namespaces seen: 0 = `user.`, 3 = `trusted.`. Others are refused, not
  guessed (open question 6).

## Open questions

Facts this reader needs that neither documentation nor black-box observation
has settled yet. Each needs a fixture that exercises it, not a guess.

1. **Packed fields that are not byte-aligned.** The aged image's formats are
   narrower (8, 16 and 32 bits, with field offsets) and are read correctly,
   but every width seen is still a whole number of bytes; the top-down bit
   order is proven only for those.
2. **Pointer device and generation bits** (assumed 48..55 and 56..63). Every
   fixture is single-device with generation 0.
3. **Btree node flags**: where the btree id and level are, and what bit 8 and
   bit 32 mean. The reader does not need them yet (it trusts the parent).
4. **Unclean filesystems**: SETTLED for single-device images (see Journal
   above). Still open: the superblock's `journal_seq_blacklist` field
   (type 8; pairs of u64, e.g. 313..4409 after a replay) on a clean image
   -- whether its end is inclusive, and whether a clean image can still
   hold bsets from a blacklisted sequence. The reader does not consult it
   yet; every clean fixture reads correctly without it.
5. **Snapshots and subvolumes**: keys are read at whatever snapshot they
   carry; visibility rules (S1 9.4) are not implemented.
6. **crc128 entries, encryption (nonces, ChaCha20/Poly1305), erasure coding,
   reflink, xattrs, multiple devices and replicas**: not seen in any
   fixture. (inline_data: seen and read, see above.)
7. **Varint fields beyond `dev`** (data_checksum ... casefold) are skipped,
   and the meaning of flags bits 32..35 of an inode's flags word is unknown.
8. **Whiteouts and deleted keys across bsets**: SETTLED for what the aged
   image holds. Nodes of up to 78 bsets merge newest-bset-wins with deleted
   keys dropped, and the result equals the lister's keys exactly. A
   `whiteout` key type has not been seen yet.
9. **Dirent names longer than one key, casefolded dirents, and the
   31-bit dirent offset change** (1.30) -- not exercised.

## Confirmation

No GPL source code (kernel `fs/bcachefs`, `bcachefs-tools`, or any crate or
snippet derived from them) was read while writing this crate. The only file
from the tools' repository that was opened is `INSTALL.md` (build
instructions, S2); the only other access was GitHub API metadata (S7).
