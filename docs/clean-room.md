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
**reference lister** (`list`), the **reference checker** (`fsck`).

Every source used is listed below with what it told us. A fact that could not
be learned this way is an **open question**, not a guess.

## Sources

| # | Source | Kind | What it told us |
|---|---|---|---|
| S1 | "bcachefs: Principles of Operation", https://bcachefs.org/bcachefs-principles-of-operation.pdf (fetched 2026-10-06) | prose documentation | Superblock at sector 8 (4 KiB), layout copy at 3584 bytes; superblock carries UUIDs, label (32 bytes), block size, btree node size, device count, version and minimum version, seq, typed variable-length fields (journal, members_v2, clean, ...); 28 btrees by name; `struct bpos {u64 inode; u64 offset; u32 snapshot}` and `struct bkey {u8 u64s; u8 format; u8 type; u8 pad; bversion; u32 size; bpos p}` as documented C declarations; packed keys use a per-node `bkey_format` with a base offset and bit width for six fields (inode, offset, snapshot, size, version_hi, version_lo) and keep a 3-byte header; extent key positions are the END of the extent; btree node = header (checksum, magic, seq, flags with btree id and level, min/max key, format, first bset) followed by `btree_node_entry` bsets; extent values are a list of entries whose type is the position of the first set bit of the first word; crc32/crc64/crc128 entry sizes (8/16/24 bytes); pointer has a device, a 44-bit sector offset and a generation; list of key types and metadata versions in order. |
| S2 | bcachefs-tools `INSTALL.md` at v1.39.7 (build instructions only) | prose documentation | The reference tools' build dependencies and minimum Rust, used only to build the oracle inside the VM. Contains no format information. |
| S3 | Reference tools v1.39.7, run in the test VM | black-box oracle | `show-super`, `list`, `fsck` output for each fixture; recorded in `.vm-share/fixtures/*.json` and `*.txt` beside the images. |
| S4 | Hexdumps of fixture images | black-box observation | See the per-structure notes below. |

## Per-structure notes

Each entry: the fact, and which source it came from.

## Open questions

Facts this reader needs that neither documentation nor black-box observation
has settled yet.
