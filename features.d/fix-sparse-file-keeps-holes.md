## Writing (`--features write`)

rename: Writing at an offset, appending, truncating to any size (the file is rewritten whole); truncating up leaves the new range a hole
| Writing at an offset, appending, truncating to any size (only the extents a write overlaps are rewritten, each whole); holes stay holes, and truncating up or writing past the end leaves the new range one | Experimental | 0.4.0 (#120); holes 0.4.2 (#156); holes kept Unreleased (#367); partial writes Unreleased (#444) | #447 (an overlapped extent is rewritten whole, not cut) | `oracle_write_local.rs`, `oracle_write_sparse.rs`, `oracle_write_partial.rs`, `oracle_write_pieces.rs`, `write_oracle.rs` |

rename: A write or truncate whose new size is larger than the device, refused before anything is allocated (the file is rewritten whole, holes as allocated zeros)
| A write or truncate whose new size is larger than the device, written sparsely: nothing past what it touches is held or allocated | Experimental | refused 0.4.2 (#196); sparse Unreleased (#444) | | `oracle_write_large_offset.rs`, `write_oracle.rs` |
