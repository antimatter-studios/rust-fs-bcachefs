## Interfaces

| `fs.bcachefs stat PATH`: the shared stat shape -- inode, type, mode, owner, links, size, allocated bytes and all four times as seconds plus nanoseconds, a symlink's target unfollowed, non-UTF-8 paths as bytes; snapshot, sectors, named flags, generation and options under `bcachefs`; device numbers null | Supported | Unreleased (#251) | #251 | `tests/cli/test-stat-tree.sh`, `write_oracle.rs` |
