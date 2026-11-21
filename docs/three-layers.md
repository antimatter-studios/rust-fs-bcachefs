# The three API layers

The rust-fs drivers share one shape: a **volume API**, a **primitive API**
that also carries the salvage verbs, and a **file API**. This page maps what
this crate already exposes onto those layers, says what each layer still
lacks, and names the pull requests that add it. The shared traits are being
added to rust-fs-core; until they are released, this crate builds each layer
as its own types, and adopts the traits in a later release with the old
entry points kept for one release as deprecated wrappers.

The rules every layer follows are the family's driver convention: primitives
decode on-disk structures and report damage without refusing it; the strict
APIs reject on top of the same primitives; salvage is a peer of the file API,
not a mode of it; and anything that reads the source takes it as
`&dyn BlockRead`, so read-only is a property of the type, not a flag.

## Volume API

Opens a device and says what volume it holds, before any directory is read.

| Exists today | Where |
|---|---|
| Superblock: identity, version, geometry, flags, features, fields | `superblock::Superblock::read`, `parse` |
| Superblock copies, highest valid `seq` wins, the layout at sector 7 as fallback | `Superblock::read_copies`, `read_layout`, `read_at` |
| Members (devices), their bucket geometry | `Superblock::members` |
| Btree roots recorded at the last clean shutdown | `Superblock::btree_roots` |
| Clean flag, never-started state | `Superblock::is_clean`, `never_started` |
| Encryption: the crypt field, sealed or unsealed master key | `Superblock::crypt`, `crypt::CryptField` |
| Usage totals | `Filesystem::usage` |
| Consistency check against the reference checker's rules | `check::check` |

Missing, and added by this work:

- **A probe that never fails on damage.** `Superblock::read` returns an error
  when no copy validates. A volume probe reports every copy the layouts name,
  each with its own state (valid, bad magic, bad checksum, unreadable), the
  copy it would choose, and where copies disagree.
- **Health of the volume as a whole**: clean or not, whether the journal must
  be replayed, member count against the devices present, encrypted or not.
- **Refusals with a reason a caller can show.** An encrypted volume is
  recognised and refused by the strict and salvage paths alike, naming whether
  a passphrase seals its key; a multi-device volume names how many devices it
  needs. `Filesystem::open` already refuses both; the probe reports them
  without opening.

## Primitive API

Read-only, damage-tolerant access to the on-disk structures, by type.

| Exists today | Where | Damage-tolerant? |
|---|---|---|
| Superblock header and fields | `Superblock::parse`, `parse_unchecked` | No: an error on bad magic or checksum |
| Superblock layout | `superblock::Layout::parse` | No |
| Journal entries in the journal buckets | `journal::read_entries`, `parse_jset` | Partly: a torn or stale entry ends its bucket |
| Journal replay over the btrees | `journal::replay`, `btree::journal_for` | No |
| Btree node: header, every bset, keys merged | `btree::Node::parse` and `read_node` | No: one bad bset refuses the node |
| Btree walk and cursor | `btree::walk`, `Cursor` | No |
| Keys: packed formats, positions | `bkey::decode`, `BkeyFormat` | Per key |
| Inodes, dirents, xattrs | `inode::Inode::from_key`, `Dirent::from_key`, `xattr` | Per key |
| Extents: pointers, checksum and compression entries | `extent::parse_entries`, `DataExtent::from_key` | Per key |
| Checksums: crc32c, crc64, xxhash | `csum::compute`, `verify` | n/a |
| Decompression: lz4, gzip, zstd | `compress` | n/a |
| Decryption: ChaCha20/Poly1305 | `crypt` | n/a |

Missing, and added by this work:

- **Superblock evidence**: every copy with its state, never an error for
  damage (`primitive::survey_superblocks`).
- **Node evidence**: a btree node read at any offset, with its header, every
  bset's checksum verdict, and every key that still decodes, the damage named
  beside them rather than refusing the node (`primitive::inspect_node`).
- **Node scan**: every btree node on the device, found by its magic, whether
  the live tree still points at it or not. bcachefs writes a changed node to a
  new place, so an older version of a node survives until its bucket is
  reused; the scan finds it and says which nodes the live tree reaches
  (`primitive::scan_nodes`).
- **Journal evidence**: every entry in the journal buckets, torn and stale ones
  included, with its sequence number, checksum verdict and the btree roots it
  records (`primitive::survey_journal`).

### Salvage verbs

The six verbs of the driver convention, on the primitives above and never on
a strict open. Nothing here repairs the source.

| Verb | bcachefs meaning |
|---|---|
| Find candidates | Inodes, from every btree node the scan finds: live, superseded, or in a node the live tree no longer reaches |
| Walk extents | An inode's extent keys, from the same nodes, as byte ranges on the device, with their checksum and compression entries; a compressed or encrypted extent is named as such, never read as plain bytes |
| Rebuild the tree | Dirents, from the same nodes, as parent claims: an inode no dirent names is an orphan, one several name is a conflict |
| Report damage | Superblock copies, btree nodes and journal entries that do not validate, and unreadable ranges |
| Read by extent | Bytes at a device range, bounded by the device |
| Materialize from a plan | A file's bytes from chosen extents, zeros and literal bytes, written to a caller's sink |
| Capabilities | What the adapter lacks, named: encrypted extents without a key, and multi-device volumes |

Snapshots: every candidate carries its snapshot id, so one inode number at
several snapshots is several candidates, never merged.

## File API

Browse, read and copy a healthy volume. This is what `Filesystem` is today.

| Exists today | Where |
|---|---|
| Open, strict, with journal replay after an unclean shutdown | `Filesystem::open` |
| Root, subvolumes and snapshots | `root`, `subvolume` |
| Path lookup, raw and casefolded names | `lookup`, `resolve`, `resolve_bytes` |
| Directory listing | `readdir`, `readdir_at` |
| Metadata | `inode`, `inode_at`, `inode_raw_at` |
| Extended attributes | `xattrs`, `xattrs_at` |
| File contents, whole or by range | `read`, `read_at`, `read_range`, `read_range_at` |

Missing: nothing a healthy read needs. When the rust-fs-core traits are
released, `Filesystem` implements the file API trait over these methods.

## Order of the work

1. rust-fs-core is taken from crates.io by version, so this crate can be
   pinned by git revision.
2. This page.
3. Primitive evidence: superblock survey, node inspection and node scan,
   journal survey, each with oracle tests on the reference tools' fixtures
   and damaged copies of them.
4. The salvage verbs on those primitives, and the volume probe.
5. The rust-fs-core traits, once released: each layer implements its trait,
   and the current entry points stay one release as deprecated wrappers.
