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
**reference lister** (`list`), the **reference checker** (`fsck`), the
**reference key editor** (`kvdb`, which reads and sets btree keys by field
name), and the **reference implementation**: the tools' userspace copy of
the filesystem mounted through FUSE (`fusemount`), which ages the `aged`
fixture inside the VM by ordinary file operations.

Every source used is listed below with what it told us. A fact that could not
be learned this way is an **open question**, not a guess.

## Sources

| # | Source | Kind | What it told us |
|---|---|---|---|
| S1 | "bcachefs: Principles of Operation", https://bcachefs.org/bcachefs-principles-of-operation.pdf (fetched 2026-10-06; title page "Revision 1.39.7+915c4e1" -- see "The specification's own licence" below) | prose documentation | Superblock at sector 8 (4 KiB), layout copy at 3584 bytes; superblock carries UUIDs, label (32 bytes), block size, btree node size, device count, version and minimum version, seq, typed variable-length fields (journal, members_v2, clean, ...); 28 btrees by name; `struct bpos {u64 inode; u64 offset; u32 snapshot}` and `struct bkey {u8 u64s; u8 format; u8 type; u8 pad; bversion; u32 size; bpos p}` as documented C declarations; packed keys use a per-node `bkey_format` with a base offset and bit width for six fields (inode, offset, snapshot, size, version_hi, version_lo) and keep a 3-byte header; extent key positions are the END of the extent; btree node = header (checksum, magic, seq, flags with btree id and level, min/max key, format, first bset) followed by `btree_node_entry` bsets; extent values are a list of entries whose type is the position of the first set bit of the first word; crc32/crc64/crc128 entry sizes (8/16/24 bytes); pointer has a device, a 44-bit sector offset and a generation; list of key types and metadata versions in order. |
| S2 | bcachefs-tools `INSTALL.md` at v1.39.7 (build instructions only) | prose documentation | The reference tools' build dependencies, minimum Rust and the `BCACHEFS_FUSE=1` switch for their FUSE mount, used only to build the oracle inside the VM. Contains no format information. |
| S3 | Reference tools v1.39.7, run in the test VM | black-box oracle | `show-super`, `list`, `fsck` output for each fixture; recorded in `.vm-share/fixtures/*.json` and `*.txt` beside the images. |
| S4 | Hexdumps of fixture images, and candidate computations over their bytes (checksum conventions) | black-box observation | See the per-structure notes below. |
| S5 | The LZ4 block format description, https://github.com/lz4/lz4/blob/dev/doc/lz4_Block_format.md (BSD-2-Clause documentation of a public format) | prose documentation | Token, literal-length and match-length encoding, little-endian 16-bit offsets, overlapping matches: the decoder in `src/compress.rs` is written from it. |
| S6 | Published check values: CRC-32C of "123456789" (0xe3069283), CRC-64/WE of "123456789" (0x62ec59e3f1a4f00a), XXH64 of the empty input with seed 0 (0xef46db3751d8e999) | public reference values | Unit-test anchors for the checksum implementations. |
| S8 | The reference implementation mounted through FUSE in the test VM (tools v1.39.7 built with `BCACHEFS_FUSE=1`, per S2), driven by ordinary file operations (`scripts/guest-age.py`) | black-box oracle | The `aged` and `aged-unclean` fixtures: what a running filesystem writes (inline data, narrow key formats, nodes of many bsets, link counts, an unclean shutdown), with the inode numbers and link counts the mount reported. |
| S9 | J.-P. Aumasson and D. J. Bernstein, "SipHash: a fast short-input PRF" (2012), https://www.aumasson.jp/siphash/siphash.pdf, and its published test vector | prose documentation | The SipHash-2-4 algorithm `src/siphash.rs` is written from. |
| S10 | The package repository index at https://apt.bcachefs.org/ and its `Packages` metadata (package names, versions, dependencies), fetched 2026-10-07 | metadata | That the reference kernel module is packaged for DKMS and needs kernel headers 6.16 or newer, and (fetched again 2026-10-09) that the `trixie` suite carries `bcachefs-kernel-dkms` 1:1.39.7, the tools' pinned version. Since #110 a Debian 13 container on the CI runner installs that package, as an oracle at arm's length: DKMS builds the module in the container, which is discarded with its source, and the module runs in a VM QEMU boots there (`scripts/kernel-oracle.sh`). No package was opened on a workstation. |
| S7 | `bcachefs-tools` GitHub API metadata (tag list, `Cargo.toml` `rust-version` field only) | metadata | Which release to pin (v1.39.7) and the minimum Rust to build it with in the VM. No source file was opened. |

## The specification's own licence

S1 is produced from the bcachefs-tools repository: its title page carries
the tools' revision ("1.39.7+915c4e1"), and it reproduces a few C
declarations from the implementation's headers (`struct bpos`, `struct
bkey`). That repository's code is GPL-2.0.

Reading it is not a GPL violation. The GPL is a copyright licence: it sets
conditions on copying, modifying and distributing the covered work, and
none on reading it. What copyright protects is *expression* -- the text of
the code and the prose -- not the facts a specification states: an offset,
a bit width, a field order, the name of a type or a feature bit. A reader
written from a specification is the ordinary clean-room route, and this
repository follows it. The exposure to manage is transcription, not
reading, so the rule for S1 is:

- **Facts, names and numbers only.** A field's position, width, order and
  name may be taken from S1 and are recorded in the per-structure notes.
- **Never transcribe.** No declaration, comment, table or passage of prose
  from S1 is copied into this crate's code, comments or docs. The Rust types
  here are field-level restatements of documented facts -- a `bpos` has
  three members in a documented order -- and nothing more.
- **Identifiers are the format's, not the implementation's.** Where this
  crate uses a name S1 uses (`bkey_format`, `btree_ptr_v2`, `inode_v3`,
  `KEY_FORMAT_CURRENT`), it is because that name is how the reference tools
  and their documentation refer to the on-disk thing, and matching the
  oracle's vocabulary is what makes the oracle tests legible.
- **Everything else is observation** (S3, S4, S8), as the notes record.

This is the authors' understanding of the position, not legal advice.

The *dependency graph* is held to the same standard mechanically:
`chore lint` runs `cargo deny check licenses` against `deny.toml`, which
allows MIT, Apache-2.0, BSD, ISC, Zlib and 0BSD and fails on anything else.

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
  btree_ptr_v2 18, inode_v3 29 (checked); also named from that order,
  not yet seen in a fixture: error 2 (reads are I/O errors, S1 9.1.2.1),
  inode 8 and inode_v2 23 (older encodings, refused by name),
  extent_whiteout 36 (reads as a hole on a filesystem without snapshots),
  reflink_p 15 and reflink_v 16 (checked, see "Reflinks" below). S1's
  order also puts indirect_inline_data at 19, which no image has shown.

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
- The root key of every btree (in the clean field, and in the journal's
  root entries) is at SPOS_MAX, the all-ones position, on every fixture
  (S4, 14 to 16 roots each): the rightmost node covers everything up to
  the end of the key space. The cursor records the root key's position as
  the btree's end rather than assuming it (`tests/oracle_cursor_end.rs`).
- A btree can have no root recorded at all: every formatter-made fixture
  records none for xattrs (3), and `aged`, which has xattrs, does (S4). The
  reference lister lists such a btree as empty, with no error
  (`write-study/base.xattrs.txt`, S3), so the reader takes a missing root
  as a btree holding nothing. For the journal's root entries the same is
  INFERRED by analogy; no fixture has shown it.

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
- Casefolded directories (#54; the `casefold` set, formatted with
  `--casefold`, S3 + S4). The superblock printer shows `casefold: 1`; every
  inode carries flag bit 10, which the lister prints as
  `has_case_insensitive` (no inode of any other image has it); `bi_casefold`
  stays 0. Each dirent's type byte has bit 7 set (0x88 for a file), then
  come two zero bytes, the name's length and the folded name's length
  (u16 each), the name as given, the folded name, and zero padding; the
  lister prints both, `Hello.TXT (casefold hello.txt)`. The folded form
  is full case folding after canonical decomposition, as far as the set
  shows: `Straße.txt` folds to `strasse.txt`, `ÉCOLE` to `e`, U+0301,
  `cole`. A dirent's slot is SipHash-2-4 of the folded name, keyed and
  shifted as for any name (all 51 entries), and the reference mount
  finds `HELLO.txt` as `Hello.TXT` (S8). The reader finds an ASCII name
  by its lowercase, which is its folded form in every case seen, and
  matches any other name as stored, by a scan; the writer refuses a
  casefolded directory (checked: tests/oracle_casefold.rs).

### Extents and data (`src/extent.rs`, `src/fs.rs`, `src/compress.rs`)

- Entry types by lowest set bit (S1); bit layouts in `src/extent.rs` found by
  hexdump against the lister's printed fields (S3, S4), checked by reading
  every file of every fixture byte for byte.
- crc128 is kind 3, three words (S3, S4: the `crc128` fixture, formatted
  with `--encoded_extent_max=1M` on 512k buckets, gave six incompressible
  extents of 832 and 1024 sectors and one lz4 extent of 2048, each a 9-u64
  key). Bits 4..16 and 17..29 are the compressed and uncompressed sizes
  less one (13 bits, S1's 8192 sectors); bits 56..59 the checksum type and
  60..63 the compression type; the second word holds the checksum the
  lister prints after the colon (the low half), the third the high half,
  0 for every checksum seen. Bits 30..42 are the offset (S8, S4: the
  `crc128-overwrite` image, a 1 MiB file written through the reference
  mount with the same options, then 4K of it overwritten: the extent's
  part after the overwrite, `offset 16`, differs from the part before
  only in 16 << 30, and the file reads back to the mount's SHA-256). Bits
  43..55 are then the 13-bit nonce S1 gives encryption alone; 0 in every
  entry seen, and a non-zero value is refused.
- The flags entry is kind 6, one word, and bit 7 is `poisoned` (S8, S4: the
  `poison` image). A data sector of a settled image was corrupted and the
  file read through the reference mount: the read failed, and the
  extent's value became `0xc0`, then its crc32 and pointer, which the
  lister prints as `flags: poisoned` (its reconcile entry was dropped). A
  later mount with reconcile on moved the file's other extents and left
  that one. The reference checker passes the image.
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
- How much is inline (#79): S1 (9.1.7) says the end of a file being
  written is stored inline when smaller than min(block_size / 2, 1024)
  bytes. The write study's `inline-*` images (one file of every size from
  1 to 2100 bytes, written through the reference mount) put the bound
  inclusive: on 512-byte blocks files of 1 to 256 bytes are inline only;
  a longer file has its full blocks in extents and its final partial block
  inline when that holds 1 to 256 bytes (1025 bytes: an extent of 2
  sectors, then inline data of 1), else nothing inline. On 4096-byte
  blocks 1 to 1024 bytes are inline and 1025 to 2100 are extents only.
  The inline key covers one block (8 sectors on 4096-byte blocks), and
  the inode's `bi_sectors` counts it with the extents' sectors (S8,
  checked for every size by tests/oracle_inline_limit.rs). A file grown
  from 100 to 3000 bytes holds no inline data afterwards (S8).

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
### The write path (`src/write.rs`, feature `write`) -- observed, then judged

- A change to a node is a new bset appended after its written part, at a
  block boundary: `btree_node_entry` csum[16], then the bset with the
  node's seq, a journal sequence, flags = checksum type | (start sector
  within the node << 16), the node's metadata version, u64s, keys (S8:
  the write study's create-small appended exactly one such bset to the
  dirents leaf, flags 0x30001 at sector 3; the aged image's nodes carry
  0x8c, 0x85, ... at sectors 140, 133, ...: bits 16.. are the start
  sector).
- The parent's `btree_ptr_v2` records `sectors_written`; the reference
  raised the root key's in the clean field from 3 to 4 (S8). The writer
  re-states each parent's pointer key in a bset appended to the parent and
  rewrites the root key in the clean field and every superblock copy.
- Keys are written unpacked (format 1): the lister's `formats` mode counts
  unpacked keys in a node, so they are allowed (S3), and the reference
  checker passes nodes holding them (checked).
- Judged: re-stating the newest key of the dirents, inodes and extents
  btrees of the write-study base and of the aged image (level-1 roots)
  leaves images the reference checker passes with nothing to fix.

### Creating a file (`Writer::create_file`) -- learned from the write study, judged

- Dirent offset = SipHash-2-4 keyed `(directory's hash_seed, 0)` over the
  name, shifted right by one. SipHash is a published algorithm (Aumasson
  and Bernstein, 2012; S9); which key and which shift were found by
  computing candidates against the write study's dirents (S4) and then
  checked against every dirent of every fixture, thousands in `aged`
  (tests/oracle_encode.rs). Collisions (open question 10) are refused.
- inode_v3 varints in order: atime, ctime, mtime, otime (two varints
  each), uid, gid, nlink, generation, dev, data_checksum, compression,
  project, background_compression, data_replicas, promote_target,
  foreground_target, background_target, erasure_code, fields_set, dir,
  dir_offset (a file stores 21 fields), then subvol, parent_subvol, nocow,
  depth (a directory stores 25). Varint encoding is the inverse of the
  decoding above; decoding then encoding every inode of every fixture gives
  back its bytes (checked).
- Times are nanoseconds since the superblock's time base (`time_base_lo`,
  nanoseconds since the epoch; precision 1) -- the reference's create was
  5.1e9 units after the base's, five seconds later (S8); the reference
  mount reports the mtime this writer sets (checked).
- The next inode number is the `inode_alloc_cursor` key (type 35) in
  logged_ops at 1:1:0, value `(u64 0, u64 next)`; the reference advanced it
  by one per create (S8).
- Accounting: `nr_inodes` at POS_MIN (value: count), and per btree the key
  at inode `0x05ffffffff000000 | id << 16` (value: keys, bytes, 0), bytes
  counted at the unpacked size (S8, every pair).
- File data is laid out as the reference lays it out (#79, above): inline
  up to min(block_size / 2, 1024) bytes, else full blocks in extents with
  a short final block inline. Data extents are cut and sized in whole
  blocks, the last zero-padded, with the checksum over the padded block
  (#87, S8: a 1500-byte file is one 8-sector extent on 4096-byte blocks,
  its crc32c that of the data and 2596 zero bytes).
  Symlink targets stay at most 248 bytes, the longest seen.
- mkdir: a 25-field directory inode with `depth` one more than its
  parent's, and the parent's stored subdirectory count raised; unlink and
  rmdir: deleted keys over the dirent, the inode and its inline data (or
  one link fewer when another name remains); rename: the old dirent
  deleted, the new one at the new name's hash, the inode's `dir`,
  `dir_offset` and ctime; rewriting an inline file: the inline key
  replaced or deleted, size, sectors and times (S8, the write study's
  mkdir, unlink, rename, truncate and overwrite pairs). Judged by the
  reference checker and mount (tests/write_oracle.rs).
- Judged: four files in the write-study base and one in the aged image
  pass the reference checker with nothing to fix, and the reference mount
  lists them and reads back their bytes, sizes and modes
  (tests/write_oracle.rs, in the guest).

### Allocating space (`src/write_alloc.rs`) -- learned by write, judge, fix

The write study's create-large pair (S8) gave the key set; the reference
checker judged each attempt and named what was missing until nothing was.

- Free space: the freespace btree (id 11) holds `set` keys (type 25) at
  `dev:end`, `size` buckets long, one per run of free buckets (S8: 0:2016
  len 1924 = buckets 92..2016). A bucket taken shrinks or removes its run.
- alloc_v4 (type 27, alloc btree id 4) per bucket: word 0 journal seq when
  it became non-empty, word 1 flags in byte 0 (0x23 on every bucket in use;
  bit 0 need_discard, bit 1 need_inc_gen), gen byte 4, oldest_gen byte 5,
  data type byte 6 (1 sb, 2 journal, 3 btree, 4 user, 9 need_discard),
  word 2 dirty sectors, word 3 read clock, word 4 write clock, word 6 the
  journal seq when it became empty (S3 + S4, field by field against the
  lister over 259 keys and the reused buckets of `aged`).
- Generations: bucket_gens (id 14) keys of type 30 at `dev:bucket/256`,
  256 one-byte generations; a pointer carries its bucket's (S3 + S4).
- Data extents: a crc32 entry (crc32c from zero, type 5; sizes minus one in
  7 bits, so at most 128 sectors) then a pointer (S4, the create-large
  extents), one bucket at most each.
- Backpointers (id 13, type 28) at `dev:sector << 16`: value btree id,
  level, data type, 5 bytes, the length in sectors u32, then the bpos of
  the key pointing at the data (S3 + S4; for a btree node, the parent key's
  position one level up).
- lru (id 10): a partly filled bucket has a `set` key at
  `(1 << 61 | dirty * 2^31 / bucket_size) : bucket` (S8, and the checker's
  "missing fragmentation lru entry at pos 2305843009549238272:101:0").
- A first lru key on a filesystem without an lru btree needs its root: an
  empty node (header flags as an existing node's with the btree id in the
  low byte, POS_MIN..SPOS_MAX, format 3 u64s 64/64/32, one empty bset), a
  btree_ptr_v2 root key at SPOS_MAX added to the clean field, and the node
  marked in the member's btree-allocated bitmap: a u64 at member offset
  128, one bit per `2^shift` sectors with the shift a byte at member
  offset 28 (S3: the printer's bitmap, most significant bit first,
  blocksize 128; the checker's "btree ptr not marked in member info btree
  allocated bitmap").
- Accounting positions are a byte string read as a big-endian bpos: kind,
  then fields (S8): 2 replicas `[data type, nr devs, nr required, devs]`
  (value: sectors), 3 dev_data_type `[dev, data type]` (buckets, sectors,
  fragmented), 5 per-snapshot btree counters (keys, bytes, data sectors),
  6 per-btree node usage (sectors, nodes), 8 inode `[inum little-endian]`
  (extents, sectors, sectors).
- Judged: files of 249 bytes to 1 MB on the write-study base and on the
  aged image pass the reference checker with nothing to fix and read back
  through the reference mount byte for byte.
- Freeing (an unlink-large pair made in the guest from create-large, S8):
  an emptied bucket's alloc key becomes data type need_discard (9), gen and
  oldest_gen one higher, need_inc_gen cleared, journal_seq_empty (word 6)
  set; the need_discard btree (id 12) gets a `set` key at
  `journal_seq_empty:bucket`; bucket_gens records the new generation;
  backpointers and lru entries go; accounting moves the bucket from user to
  `dev_data_type need_discard` (and keeps zeroed keys, as the reference
  does). Judged: unlinking, shrinking and growing large files passes the
  reference checker and the mount reads what remains (S8 checker run).
- A bucket holding several nodes (#109): on a filesystem whose nodes are
  smaller than its buckets the reference formatter fills a bucket with
  more than one node (`write-study/base-small-nodes.img`: 64-sector nodes
  in 256-sector buckets, 17 of its 18 btree buckets holding more than one
  node's sectors). Freeing one of them takes only its sectors from the
  bucket's dirty count and one node from the btree's; the bucket is
  emptied, as any emptied bucket is, only when none is left. This writer
  puts one new node in each bucket. A partly filled btree bucket has a
  fragmentation lru entry, as a partly filled data bucket does, at
  `(1 << 61 | dirty * 2^31 / bucket_size) : bucket` (the reference
  checker's "missing fragmentation lru entry" and "incorrect lru entry" on
  such an image, CI run 38051787458): a new node's bucket gets one, and
  freeing a node moves it.
- Reusing a freed bucket (#132; the reference kernel module's
  `kernel-freed` image, S10: a 4 MiB file written, removed, and ten
  seconds before the unmount, made once by an observation build of the
  kernel oracle in CI run 38017596962, whose `fixtures` artifact keeps the
  reference lister's view of its alloc, need_discard, freespace,
  bucket_gens and accounting btrees): the reference returns a need_discard
  bucket to the free pool on a device it does not discard. Its alloc key
  becomes data type free with gen and oldest_gen kept, need_discard
  cleared and both journal sequence numbers (words 0 and 6) 0, io_time
  kept; its need_discard key goes; freespace holds it again, merged into
  the run beside it (buckets 89 and 90, both gen 1, as one run `0:91 len
  2`); accounting moves it from `dev_data_type need_discard` to `free`.
  The writer does this before any write that allocates, for buckets
  emptied by earlier commits, giving the freed buckets runs of their own
  rather than merging them into an existing run: a node the same commit
  rewrites takes its bucket from the runs on disk and rewrites the run it
  shrinks under the same key, which replaced a merged run and lost the
  buckets merged in (the reference checker, CI run 38024306750: "bucket
  incorrectly unset in freespace btree"). Judged: a 4 MiB file written and removed 25
  times through the 64 MiB base image passes the reference checker and
  reads back through the reference mount (`write_oracle.rs`). Not seen:
  whether a freespace key's position carries a generation's high bits
  once it reaches 16, so buckets of generation 16 or more are left
  waiting.

### Committing through the journal (`src/write_journal.rs`) -- judged

- A transaction is one jset in the layout the reader decodes: seq one past
  the newest entry, last_seq the session's first, flags checksum type |
  0x40 (bit 6 set on every flush entry the reference wrote), a btree_keys
  entry per btree and a btree_root entry per root, at the next free block
  of the journal ring (S4, S8).
- Accounting goes in as signed deltas with version (seq, position), which
  the replay adds once to keys of a lower version (S8: the reference's own
  journal carries `replicas user ... -14`, `inum -1 -14 -14`).
- The superblock's clean bit is cleared after the first entry, and its
  replicas_v0 field lists the journal on the device (`02 01 00`; S4, and the
  checker's "superblock not marked as containing replicas for journal
  entry", its only finding on the first attempt).
- Nothing is reclaimed: no entry's last_seq ever passes the session's
  first, so every entry since then is in the replay window. The ring may
  not come round into the bucket holding the window's first entry; the
  entry that would is refused as "the journal is full" (#101). Reclaim,
  as S1 9.7.2 describes it (write the nodes the oldest entries pin, then
  advance last_seq), is #113.
- Judged: entries 13-16 written to the write-study base (a small file, a
  large file with a new lru root, a mkdir, an unlink) are replayed by the
  reference checker with nothing to fix and read back through its mount;
  an entry left unmarked is ignored by both.

### Links, attributes and extended attributes (`Writer::symlink`, `link`, `set_attributes`, `set_xattr`) -- judged

- A symlink is an inode of mode 120777 whose target is its inline data,
  size the target's length, dirent type 10 (S8: the aged fixture's
  symlinks).
- A hard link adds a dirent; the inode's stored count rises and its
  `dir`/`dir_offset` back-reference moves to the newest name (S8: the aged
  fixture's three-name file points at the last one made).
- Permissions are the mode bits in the flags word; owner and group the uid
  and gid varints (S8 field order, above).
- An xattr's key sits at `xattr::name_slot`: SipHash-2-4 keyed `(inode
  hash_seed, 0)` over the namespace byte then the name, shifted right by
  one, with the final partial word of a longer message taken as question
  15 records (#77; every xattr of every image checked). The first xattr of a filesystem
  without an xattrs btree makes its root, as for lru.
- An empty bset is rejected by the reference checker ("empty bset"), so
  nothing is appended when an operation leaves a btree unchanged (S8,
  measured on the first attempt).
- Judged: a symlink, a second name, chmod and chown, and xattrs set,
  replaced and removed pass the reference checker, in place and through
  the journal, and the reference mount and `getfattr` show them.

### Rewriting and splitting full nodes (`src/write_nodes.rs`) -- judged

- A full node is rewritten into a fresh bucket with its live keys in one
  bset, or split in two when they would fill more than two thirds of a
  node; the parent's pointer keeps its position (the node's max key), a
  split adds one before it, and a root that splits gets a new root one
  level up (its level byte in the clean field's entry follows). The old
  bucket is freed as an emptied data bucket is (need_discard, a new
  generation), its backpointer removed.
- Node header flags: the btree id's low four bits in bits 0..4, the level in
  bits 4..8, bit 8 set, the id's higher bits from bit 9 (S4: every root of
  the aged fixture, ids 0 to 20; id 16 is 0x300, id 20 is 0x304).
- The btree-allocated bitmap covers 64 regions of 2^shift sectors; a node
  beyond them doubles the regions (shift + 1, bits folded pairwise) until
  it fits. The reference checker accepts the superset and schedules its
  own bitmap pass ("has 672k btree buckets and 3.25M marked in bitmap").
- The per-btree accounting key's third counter counts interior nodes (the
  checker's "btree btree=inodes ... should be 256 4 1" after a root split;
  the aged fixture's two-level btrees carry 1).
- Judged: 400 one-operation creates, 134 unlinks and ten 50 KB files on the
  write-study base (32 KiB nodes) pass the reference checker and read back
  through its mount.

## Open questions

Facts this reader needs that neither documentation nor black-box observation
has settled yet. Each needs a fixture that exercises it, not a guess.

1. **Packed fields that are not byte-aligned** (#82). SETTLED as "the
   reference does not write them" (S3): in the 125 node formats of `aged`
   and `large` alone, 588 fields, every width is 0, 8, 16, 32 or 64 bits,
   including 16-bit inode fields over ranges of about 470 that 9 bits would
   hold. The reference rounds a field's width up to whole bytes. Every
   format of every fixture and write-study image is read node for node as
   the lister prints it, and every width checked to be whole bytes
   (tests/oracle_formats.rs). The unpacker's handling of other widths is
   tested only against this crate's own packing (self-consistency, not
   correctness); no reference image can exercise it.
2. **Pointer device and generation bits** (48..55 and 56..63): the
   generation bits are now checked -- every pointer of every fixture,
   including the aged image's nine pointers of generation 1 into reused
   buckets, carries the generation its bucket's `alloc_v4` key records
   (`tests/oracle_pointers.rs`); the device bits are only ever 0. The
   reader refuses a pointer whose generation is not the bucket's (stale,
   S1 9.1.3.1), one for another device, or one with any flag bit set (bits
   1..3; cached and unwritten pointers, never observed). Btree node
   pointers are not yet judged the same way.
3. **Btree node flags**: what bit 8 and bit 32 mean. The btree id and
   level are settled (question 13); the reader does not need them (it
   trusts the parent), and the writer copies the other bits from an
   existing node.
4. **Unclean filesystems**: SETTLED for single-device images (see Journal
   above), and the superblock's `journal_seq_blacklist` (type 8) is
   SETTLED too: pairs of u64 `(start, end)` with the end EXCLUSIVE. S1
   (1.3, 9.7.5): bsets referencing a blacklisted sequence "are ignored
   until the btree node is next rewritten", and after an unclean shutdown
   64 sequence numbers past the last journal entry read are blacklisted
   too, so a clean image can hold such bsets. Observed (S3, S4): the aged
   image's field reads 307..4403 after the reference replayed entries
   304-306; no bset of its nodes has a journal sequence inside the range,
   two have exactly 4403, and the lister reads their keys -- so 4403 is
   live and the end is exclusive. The reader ignores bsets in
   `start <= seq < end` (`Node::parse_filtered`). Still open: the journal's
   own `blacklist` and `blacklist_v2` entries (types 3, 4) -- none in any
   fixture's journal; a replay window holding one is refused.
5. **Snapshots and subvolumes** (#12) are answered for reading; see
   "Snapshots and subvolumes" below. No route through the reference FUSE
   mount makes one, and the reference key editor fabricates snapshot keys
   but never a whole subvolume the checker passes (#114), so the
   reference tool makes them in the kernel oracle's VM. Still open: what
   the low half of a snapshot key's first word and of a subvolume key's
   first word hold (2 and 0 seen; the lister prints `live` and, for a
   snapshot of a subvolume, `snapshot`), and the skiplist's use.
6. **Encryption (nonces, ChaCha20/Poly1305), erasure coding, reflink,
   multiple devices and replicas**: not seen in any fixture read by this
   crate. (inline_data, xattrs and crc128: seen and read, see above.)
   **Reflink** (#7) is answered; see "Reflinks" below. No route through
   the reference FUSE mount makes one (FICLONE, FICLONERANGE and
   FIDEDUPERANGE are refused before the daemon sees them;
   `copy_file_range` copies), so the reference kernel module makes it.
7. **Varint fields beyond `dev`** (#81). Their names and order are
   SETTLED (S3): every field of every inode of every dumped image decodes
   to the value the lister prints as `bi_<name>`
   (`inode::FIELD_NAMES`, `InodeV3Raw::field`,
   tests/oracle_inode_fields.rs). What the option fields mean is SETTLED
   for `data_checksum`, `compression`, `background_compression` and
   `data_replicas` (S3 + S4, #81): the reference tool, run in the kernel
   oracle's VM (S10), set each with `set-file-option` on an empty file,
   which was then written, and once on a directory, which a new file was
   created in. What `get-file-option` printed (`kernel.options.txt`) and
   each inode's fields:
   - each field holds the option's value plus one, 0 when unset:
     `data_checksum` none 1, crc64 3, xxhash 4 (so crc32c 2);
     `compression` and `background_compression` lz4 2, gzip 3, zstd 4 (so
     none 1, the filesystem option's numbering plus one); `data_replicas`
     1 is stored as 2;
   - `fields_set` has one bit per option set on the inode itself, counted
     from `data_checksum` in field order (data_checksum 1, compression 2,
     background_compression 8, data_replicas 16);
   - a file created in a directory with an option inherits it: the field
     is stored (`compression` 4) with no `fields_set` bit, and
     `get-file-option` lists it not;
   - the data follows the option: every extent of the lz4, gzip and zstd
     files, the inherited one included, is compressed with that codec
     (tests/oracle_kernel.rs);
   - setting an option also sets inode flag bit 12 (0x1000), on the
     inherited file too; what that bit is called is open.
   Still open: the other option fields (`promote_target`,
   `foreground_target`, `background_target`, `erasure_code`, `nocow`,
   `project`), which no image sets; and a compression level (`zstd:3`).
   The routes that showed nothing:
   - the FUSE mount refuses every `bcachefs.*` option xattr, and the
     reference tool's `set-file-option` through it, with "Operation not
     supported";
   - an option given to the formatter is not copied into any inode: every
     inode of the lz4, zstd, gzip, crc64, xxhash and bgcompress sets has
     its option fields 0, and of the casefold set too (`casefold: 1` in
     the superblock, `bi_casefold=0` in every inode);
   - mount options are filesystem-wide (S1 7.1), none per inode;
   - the reference tool's offline key editor (`kvdb`, S1 6.12) edits an
     inode only up to its fixed header in the pinned release: `update
     inodes <pos> bi_compression=2` answers "bch_inode_v3 has no field
     'bi_compression'" (write-study/kvdb-options.txt).
   The writer leaves the option fields 0 and writes data as the
   filesystem's options say, whatever an inode carries.
8. **Whiteouts and deleted keys across bsets**: SETTLED for what the aged
   image holds. Nodes of up to 78 bsets merge newest-bset-wins with deleted
   keys dropped, and the result equals the lister's keys exactly. A
   `whiteout` key type has not been seen yet.
9. **Dirent names longer than one key, and the 31-bit dirent offset
   change** (1.30) -- not exercised. (Casefolded dirents: question 18.)
10. **Dirent hash collisions** (#78). SETTLED by observation (S8). SipHash
    slots are 63 bits wide, so the write study collides names under
    `--str_hash=crc32c` instead: four 16-byte names whose XOR differences
    have a CRC of 0 collide under any seed. Created in order through the
    reference mount, they took four consecutive offsets in creation order.
    Removing the second left a `hash_whiteout` (u64s 5, no value) in its
    slot, and the others stayed. The dirents counter kept 7 keys, its bytes
    fell by 72 - 40, so a whiteout is a counted 40-byte key. Created again,
    the name took the whiteout's slot. A removal with nothing after it (the
    write study's unlink) deletes the key outright. So a name is looked up
    from its hash slot up, past other names and whiteouts, to an empty
    slot; the reader and the writer both do this. INFERRED for SipHash
    directories, where no collision can be made: the same rule.
13. **Node flags for btree ids of 16 and more** (#80). SETTLED (S8): a
    node's flags carry the id's low four bits in bits 0..4, the level in
    bits 4..8 and the id's higher bits from bit 9, for every node of every
    btree of every clean fixture and write-study image, ids 16 and above
    included (`btree::flags_id_and_level`, tests/oracle_node_flags.rs).
    The writer builds a new root's flags that way for any id. No write it
    makes yet needs a root for an id of 16 or more, so no such root has
    been judged by the reference checker.
12. **Flags bits 32..35 of an inode** are 3 in every inode of every
    dumped image (S4, tests/oracle_inode_fields.rs; #81); what they mean
    is open, and the writer copies them from the parent, which keeps them
    3. The accounting keys' versions are copied unchanged too; the
    reference checker accepts both.
15. **The xattr slot** (#77). SETTLED, INFERRED and checked (S8). The two
    of `aged`'s xattrs that did not fit (`user.on-a-directory`,
    `user.greeting`) were not about when they were set: their inodes'
    seeds never changed, and the same names misfit again on fresh images.
    One xattr of every name length from 1 to 24 showed the rule: the
    message is the namespace byte then the name, and SipHash-2-4 keyed
    `(hash_seed, 0)` over it, shifted right by one, gives the slot, except
    that a message longer than 8 bytes and not a whole number of words has
    a final partial word of a zero byte then all but the message's last
    byte. Names of 1 to 7, 15 and 23 bytes therefore fit the plain hash;
    the rest do not. `xattr::name_slot`, checked on every xattr of the
    write study's xattr images and all 45 of `aged`
    (tests/oracle_xattr_slots.rs).
16. **Superblock copies.** SETTLED. S1 (9.5.1): the copy with the highest
    valid `seq` is authoritative, and the standalone layout at sector 7 is
    consulted when the primary cannot be read. Observed (S4): every fixture
    carries three copies, at sectors 8 and 2056 and at the end of the
    device, each recording its own offset, all with one `seq`. The reader
    reads every copy and takes the highest `seq`, falling back to the
    layout at 3584 when the primary does not read; the checker reports a
    copy that does not read (`tests/oracle_superblock_copies.rs`).
17. **The string hash.** S1 (7.7): the `str_hash` option is one of crc32c,
    crc64 or siphash (the default), for dirents and xattrs alike; the
    inode's flags bits 20..23 carry the hash type. SipHash is 3 (S3, S4:
    every inode of every fixture, and the lister's `hash_type=siphash`).
    The reader scans a directory of any other type instead of hashing
    (`Filesystem::find`), the writer refuses to place a name in one
    (`siphash_only`), and the `strhash` fixture (`--str_hash=crc32c`) is
    read whole by scanning (`tests/oracle_strhash.rs`, which prints the
    number crc32c carries). crc32c is hash type 0, and its dirent offset is
    CRC-32C (initial value all ones, no final XOR) over the directory's
    `hash_seed` as eight little-endian bytes followed by the name
    (INFERRED by computing candidates against the write study's crc32c
    images, then checked against all 314 dirents of `strhash`:
    `inode::name_hash`, tests/oracle_collisions.rs). The xattr hash under
    crc32c is SETTLED (S8, #106): the same CRC over the seed, then the
    namespace byte and the name, with none of SipHash's rearranged final
    word; every xattr of the write study's xcollide images sits there, and
    four colliding names take that slot and the three after it, in the
    order set (`xattr::slot`, tests/oracle_xattr_slots.rs). Still open:
    crc64's number and inputs.
18. **Casefolded directories.** SETTLED for what the reader needs (see
    Inodes and dirents above). The reference mount refuses `chattr +F` and
    `set-file-option --casefold` on a directory (`probe.txt`), but the
    formatter takes `--casefold` for the whole filesystem, and its mount
    then folds lookups. Still open: the folding of names beyond ASCII
    (Unicode's tables, "utf8-12.1.0" by the superblock printer's own
    words), which a lookup of such a name would need to hash, so the
    reader scans for it as stored (issue #111); whether
    `has_case_insensitive` alone marks a directory, or only with the
    superblock option, since no image has one without the other; and what
    bytes 9..10 of a casefolded dirent hold, 0 in every one seen and
    refused otherwise.
19. **Extent entries beyond ptr, crc32 and crc64.** S1 (9.1.3, 9.1.12)
    names crc128 (24 bytes, required under encryption), stripe pointers
    (erasure coding), a flags entry (poisoned) and a `reconcile` entry
    recording IO options, e.g. `[crc32, ptr, ptr, reconcile]`.
    **Reconcile is answered by the `bgcompress` fixture**
    (`--background_compression=lz4`, S3, S4): its listing shows
    `reconcile: need_rb=background_compression replicas=1 checksum=crc32c
    background_compression=lz4` after the pointer of 302 of its 440
    extents; those keys are 8 u64s against 7 for the same `[crc32, ptr]`
    without it, and the extra word in the image is
    `0x0000_0010_9010_0080`, whose first set bit is 7. The reader passes
    over that one word and reads the data; the files read back to the
    mount's SHA-256 (`tests/oracle_bgcompress.rs`). Still open: the
    word's fields, and whether it can be longer with other options set.
    **crc128 (kind 3) and flags (kind 6, poisoned at bit 7) are answered**
    by the `crc128` fixture and the `crc128-overwrite` and `poison` images
    (see Extents above); a crc128 nonce, only ever 0, is refused unless 0.
    **The stripe pointer is kind 4**, one word (S8, S3, S4: the fixture
    build's erasure-coding probe, three devices, `--erasure_code
    --replicas=2`, `probe-ec.txt`). The reference first writes each extent
    with two pointers, then rewrites it as `[crc32, stripe_ptr, ptr]` keys
    of 8 u64s once the stripe is made; in that copy `stripe_ptr: idx 1
    block 0` is the word 0x22010, `idx 1 block 1` 0x22030 and `idx 2 block
    0` 0x42010, through `idx 4`. So bits 5..12 are the block and bits
    17..63 the stripe index; bits 13..16, which the lister does not
    print, hold 1 in every one (S1 names a 4-bit redundancy). This reader
    reads one device, so a stripe pointer is refused by name. Kind 5 is
    the one kind no image has shown.

20. **gzip-compressed extents this writer makes** (#105). The writer
    stores data on an lz4 or zstd filesystem compressed, and the reference
    mount reads it back byte for byte (CI run 38013718306). On a gzip
    filesystem it wrote each piece as a raw deflate stream
    (`miniz_oxide`, level 6), the framing this crate decodes from the gzip
    fixture; the reference checker passed the image, and this crate reads
    it back, but the reference mount's daemon died reading it
    (`bcachefs: fatal SIGSEGV` in `fuse_read`, same run). Whether the
    stream differs from what the reference writes, or its userspace gzip
    path fails on any stream, is open. Until the reference kernel module
    judges such an extent, data on a gzip filesystem is written as
    incompressible.

### Reflinks -- read

The reference kernel module (S10, `scripts/kernel-oracle.sh`) clones a
3 MiB file with FICLONE; the reference lister's view of the extents and
reflink btrees is kept as `kernel.extents.txt` and `kernel.reflink.txt`.

- Both files' extents become `reflink_p` keys (type 15, 7 u64s: a 16-byte
  value), each covering a range of the file as an extent does. The lister
  prints `idx N front_pad 0 back_pad 0 may_update_opts`: the first word's
  low 56 bits are `idx` (S1 9.1.6 says 56 bits) and bit 57 is
  `may_update_opts`; the second word is 0 with both pads 0, so where each
  pad sits in it is still open.
- The shared data is in the reflink btree (id 7) as `reflink_v` keys
  (type 16) at `0:end`, `size` sectors long, like an extent: a u64
  refcount (2 for a file and one clone), then an extent's entries
  (`crc32`, then `ptr`).
- A `reflink_p` at file sectors `start..end` reads the reflink btree's
  sectors `idx..idx + (end - start)`.
- Judged: every file the kernel wrote, the clone and its source included,
  reads back here at the SHA-256 its mount reported, and every pointer's
  index and flag match the lister (`tests/oracle_kernel.rs`). The checker
  does not yet follow a reflink, and the writer will not free one (it
  would have to drop the shared extent's refcount).

### Snapshots and subvolumes -- read

The reference tool, run in the kernel oracle's VM (S10,
`scripts/kernel-oracle.sh`), makes a subvolume `sv`, fills it, snapshots
it as `snap`, then changes, removes and adds a file in `sv`. The lister's
view of the subvolumes, snapshots, snapshot_trees, inodes, dirents and
extents btrees is kept as `kernel.<btree>.txt`.

- Subvolumes (btree 8, key type 21) at `0:id`: the first word's high half
  is the subvolume's snapshot and the second word its root inode (`sv`: 2,
  snapshot 4294967292, root 2305843009213693952; `snap`: 3, snapshot
  4294967293, the same root inode). The third word holds
  `creation_parent` in its low half and `fs_parent` in its high half.
- Snapshots (btree 9, key type 22) at `0:id`: the first word's high half
  is the parent (0 for a tree's root), the second word the two children,
  the third `subvol` in its low half and `tree` in its high half. Taking
  the snapshot made 4294967294 the interior parent of the two leaves
  4294967292 (`sv`) and 4294967293 (`snap`); the keys written before it
  are at 4294967294.
- A dirent of type 16 (`subvol`) names a subvolume: the low half of its
  first word is the subvolume and the high half its parent (the lister's
  `sv -> 1 -> 2`).
- Visibility: a snapshot sees keys at its own id and its ancestors'; at
  one position the nearest wins. `new.txt`'s dirent and the changed
  file's new inode and inline data are at `sv`'s 4294967292, beside the
  old ones at 4294967294; the removed `gone.txt` is a
  `whiteout` (key type 1) there over its dirent at 4294967294, which
  `snap` still sees. A file keeps its inode number in every snapshot.
- Judged: every file, directory and symlink in the root subvolume, in
  `sv` as it is now and in `snap` as it was reads back at the inode
  number, mode and SHA-256 the kernel's mount reported
  (`tests/oracle_kernel.rs`). The writer and the checker still read one
  snapshot, the root subvolume's.

## Confirmation

No GPL source code (kernel `fs/bcachefs`, `bcachefs-tools`, or any crate or
snippet derived from them) was read while writing this crate. The only file
from the tools' repository that was opened is `INSTALL.md` (build
instructions, S2); the only other access was GitHub API metadata (S7) and
the package repository's index and dependency metadata (S10). The
reference kernel module is built by DKMS in a throwaway container on the CI
runner and its source discarded with it, unread (#110).
