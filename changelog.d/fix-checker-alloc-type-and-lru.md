### Fixed

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
