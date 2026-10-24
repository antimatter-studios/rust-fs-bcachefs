## Interfaces

| `fs.bcachefs` reading verbs (`ls`, `ls -R`, `stat`, `read`, `export`, `export --recursive`) on a missing path: exit 1 with `"kind": "not_found"` in the JSON error, as the CLI contract has it | Supported | Unreleased (#404) | #404 | `cli::fs::tests::a_missing_path_is_the_not_found_kind`, `tests/cli/test-read.sh`, `test-stat-tree.sh`, `test-ls-recursive.sh` |
