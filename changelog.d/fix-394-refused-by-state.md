### Added

- **`Error::is_state_refusal`** tells the volume's state refusing a change
  to its names (a name not there, a name already taken, a directory that is
  not empty, a name that is not a directory where one is needed) from
  damage and from this crate's limits (#394).

### Changed

- **`fs.bcachefs`'s namespace verbs exit 4 when the volume's state refuses
  them (#394).** `mkdir`, `rm`, `rmdir`, `mv` and `ln` return `refused`,
  exit 4, with `"code": 4` in the JSON error, for those refusals. They
  returned 1 before. Every other failure keeps its status, as the CLI
  contract has it.
- **Built on rust-fs-core 0.3.17**, for `CliError::refused_by_state`.
