## Interfaces

| `fs.bcachefs export PATH DEST`: one regular file, or with `--recursive` a directory tree through the shared tree transfer, subvolumes and snapshots entered, each item reported; `--preserve=links,times` keeps hard links, symlink targets (never followed) and the stored access and modification times (`--features cli`) | Supported | Unreleased (#342); exit-status compatibility Unreleased (#714) | #706 | `tests/cli/test-export-tree.sh`, `tests/cli/test-export.sh`, `cli_export_tree_args.rs`, `cli::fs::tests::tree_export_preserves_its_status_and_report_contract` |
