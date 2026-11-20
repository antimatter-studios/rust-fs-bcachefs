## Reading

| Unwritten extents (preallocated in a `nocow` file), read as zeros; `fallocate` elsewhere makes reservations, also read as zeros | Supported | Unreleased (#596) | #158, #575; cached pointer flag 0b001 is refused; the meaning of accepted flag 0b010 remains open | `oracle_unwritten.rs`, `check_oracle.rs::both_checkers_agree_on_every_extent_pointer_flag_value` |

## Checking

| All eight extent pointer flag values are judged against the reference checker without treating flag 0b010 as cached | Supported | Unreleased (#596) | #575; #230 remains unresolved and its backpointer checks are preserved | `check_oracle.rs::both_checkers_agree_on_every_extent_pointer_flag_value` |
