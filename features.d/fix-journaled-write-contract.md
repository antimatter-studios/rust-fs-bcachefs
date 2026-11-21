## Writing (`--features write`)

| A journalled `put` or `rm` that cannot fit one journal entry is refused before changing the visible file | Refused: the old file remains readable after the refusal | Unreleased (#667) | #657 | `oracle_write_local.rs`, `write_oracle.rs`, `tests/cli/test-write-docs.sh` |
