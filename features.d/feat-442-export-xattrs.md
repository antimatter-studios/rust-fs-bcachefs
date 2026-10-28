## Interfaces

| `fs.bcachefs export --preserve=xattrs`: every file's and directory's extended attributes, name and value, carried to the host for a tree or a single file; an attribute the host cannot hold (`trusted.*` without privilege) refuses its item by name with a nonzero exit, never dropped (`--features cli`) | Supported | Unreleased (#457) | #285 (import, #443) | `tests/cli/test-export-xattrs.sh`, `cli_export_tree_args.rs` |
