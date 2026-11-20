## Interfaces

| Salvage: find candidates, every inode in every inodes-btree node on the device, live or superseded by an older node version, with its node, bset and checksum verdict (`salvage::Salvage::find_candidates`) | Supported | Unreleased | | `oracle_salvage.rs` |
| Salvage: walk extents, a candidate's extent keys from every node version as file pieces, snapshots' ancestors and reflinked data followed, holes, conflicts and unreadable ranges named (`Salvage::walk_extents`) | Supported | Unreleased | | `oracle_salvage.rs` |
| Salvage: rebuild the tree from every directory entry found, orphans and conflicting parents named, hard links allowed up to the link count (`Salvage::rebuild_tree`) | Supported | Unreleased | | `oracle_salvage.rs` |
| Salvage: report damage to superblock copies, the layout, journal entries, btree nodes and live-tree pointers, and unreadable ranges (`Salvage::report_damage`) | Supported | Unreleased | | `oracle_salvage.rs` |
| Salvage: read by extent and materialize from a plan to a caller's writer, every output range named by its source, and data that does not decode reported missing, never passed off as zeros (`Salvage::read_extent`, `plan`, `materialize`) | Supported | Unreleased | | `oracle_salvage.rs` |
| Salvage of an encrypted or multi-device volume | Refused | Unreleased | #690 | `oracle_salvage.rs` |
