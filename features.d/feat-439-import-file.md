## Interfaces

| `fs.bcachefs import SOURCE PATH`: one local file or stdin, staged beside PATH, committed through the journal and published by one rename; `--replace` replaces in the same transaction, `--verify` reads back through a fresh open (`--features cli`) | Supported | Unreleased (#439) | #283 | `tests/cli/test-import.sh`, `write_oracle.rs` (`cli_import*`) |
