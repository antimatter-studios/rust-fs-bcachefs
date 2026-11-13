## Checking

| `fsck.bcachefs` on an extent's checksum type: a crc entry of type none, crc32c, crc64 or xxhash other than the `data_checksum` option in force (the inode's, else the filesystem's), on an extent with no reconcile entry, is reported as the reference checker reports it; an extent with no crc entry, and an unwritten, poisoned or nocow one, are not judged | Supported | Unreleased (#619) | #614 | `check_oracle_extent_checksum.rs`, `extent_checksum_tests.rs` |
