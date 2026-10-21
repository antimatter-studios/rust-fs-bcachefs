## Writing (`--features write`)

rename: Writing at an offset, appending, truncating to any size (the file is rewritten whole); truncating up leaves the new range a hole
| Writing at an offset, appending, truncating to any size (the file is rewritten whole); holes stay holes, and truncating up or writing past the end leaves the new range one | Experimental | 0.4.0 (#120); holes 0.4.2 (#156); holes kept Unreleased (#367) | #366 (a write rewrites the whole file) | `oracle_write_local.rs`, `oracle_write_sparse.rs`, `write_oracle.rs` |

rename: A write or truncate whose new size is larger than the device, refused before anything is allocated (the file is rewritten whole, holes as allocated zeros)
| A write or truncate whose new size is larger than the device, refused before anything is allocated (the file is held in memory whole) | Refused | 0.4.2 (#196) | #366 (writes that touch only the extents they overlap) | `oracle_write_large_offset.rs` |
