## Interfaces

rename: Inode times through the C ABI (`fs_bcachefs_stat_times`): access, modification and change time since the epoch; the birth time is not read
| Inode times through the C ABI (`fs_bcachefs_stat_times`): access, modification and change time since the epoch, including times before the superblock time base; the birth time is not read | Supported | Unreleased (#374) | #307 | `oracle_capi.rs`, `write_oracle.rs` |
