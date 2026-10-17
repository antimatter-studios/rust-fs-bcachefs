### Added

- `fs.bcachefs get [KEY]` reports the shared properties as JSON, or text
  with `--text`; dotted keys select filesystem-specific fields.
- `set KEY VALUE` and `resize SIZE [--force]` answer unsupported operations
  with exit 3 before opening the target, preserving its bytes.

### Changed

- `info [KEY]` is identical to `get`: the ordered shared envelope is `fs`,
  `label`, `total_bytes`, `free_bytes`, `block_size`, `dirty`, `bcachefs`.
  Previously top-level `uuid`, `version`, `btree_node_size`, `devices`,
  `encrypted` and `clean` now live under `bcachefs`; an empty label is null.
  Full reports read filesystem trees to count free bucket runs.
