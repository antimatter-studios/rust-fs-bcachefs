### Fixed

- A journal reclaim by a writer that did not take the buckets the journal's entries hold no longer writes a btree node over a file's data, and no longer fails with `bucket N is free and has an alloc key`.
- A journal reclaim changes only free buckets until the one superblock write that ends it, so a reclaim that stops part way leaves the journal to replay, and its writer commits nothing more. It used to mark the filesystem clean with only some of the keys in their nodes, which lost the rest.
- A journalled session reclaims its journal before its entries take the last quarter of the journal's buckets, at least two, because a replay needs journal space of its own.
- `fs.bcachefs import` of a file larger than one journal entry holds succeeds: it commits in runs that fit one entry.
- A failed `fs.bcachefs import` removes a staged file of any size, over as many commits as that needs.
- Buckets freed by removing a large amount of data are made free over as many commits as they need, where one commit was refused as too large for a journal entry.

### Changed

- Data written in a journalled session leaves free, beyond the 32 buckets kept for btree nodes, the buckets a reclaim of the whole journal needs: 25 on 32 KiB journal buckets and 32 KiB nodes with 16 journal buckets.

### Added

- `Writer::largest_journalled_write` and `Writer::unlink_in_steps`.
