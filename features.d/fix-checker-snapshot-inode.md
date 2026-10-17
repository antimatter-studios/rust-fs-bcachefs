## Checking

| `fsck.bcachefs` on snapshot key positions: a snapshot key in the snapshots btree with a nonzero inode position | Supported | Unreleased (#269) | #253 | `oracle_check_key_positions.rs`, `check_oracle.rs`, `check.rs` unit tests |
| Snapshot-tree keys with nonzero inode positions | Supported | Unreleased | #254 | `oracle_check_key_positions.rs`, `check_oracle.rs`, `check.rs` unit tests |
| Inode keys with nonzero inode positions | Not supported for one measured disagreement class | | #255 | `differential.yml` report 39018152705 |
| Backpointer values that disagree with their extents | Not supported for one measured disagreement class | | #256 | `differential.yml` report 39018152705 |
| Logged-op-truncate keys in the freespace btree | Not supported for one measured disagreement class | | #257 | `differential.yml` report 39018152705 |
| Xattr keys with nonzero sizes | Not supported for one measured disagreement class | | #258 | `differential.yml` report 39018152705 |
| LRU keys in the xattrs btree | Not supported for one measured disagreement class | | #259 | `differential.yml` report 39018152705 |
| Superblock-copy findings after superblock mutations the reference checker accepts | Unresolved disagreement | | #261 | `differential.yml` report 39018152705 |
| Journal entry versions the reference checker rejects | Not supported for one measured disagreement class | | #262 | `differential.yml` report 39018152705 |
| Inode keys in the logged-ops btree | Not supported for one measured disagreement class | | #263 | `differential.yml` report 39018152705 |
| LRU keys in the freespace btree | Not supported for one measured disagreement class | | #264 | `differential.yml` report 39018152705 |
| Snapshot-tree subvolumes that do not point into their trees | Not supported for one measured disagreement class | | #265 | `differential.yml` report 39018152705 |
| Logged-op-truncate keys in the subvolume-children btree | Not supported for one measured disagreement class | | #266 | `differential.yml` report 39018152705 |
| Snapshot-tree roots that point to missing or incorrect snapshots | Not supported for one measured disagreement class | | #267 | `differential.yml` report 39018152705 |
| Inode keys in the backpointers btree | Not supported for one measured disagreement class | | #268 | `differential.yml` report 39018152705 |
