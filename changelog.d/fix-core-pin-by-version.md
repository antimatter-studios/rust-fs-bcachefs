### Fixed

- **This crate can be depended on by git revision.** `rust-fs-core` is taken
  from crates.io by version (`0.5`) instead of a path to a sibling checkout,
  which made a git dependency on this repository fail to resolve. Local
  development against an unreleased `rust-fs-core` overrides it in an
  untracked `.cargo/config.toml`.

### Changed

- **rust-fs-core 0.5.0.** The family's scripts, workflows and the crate itself
  move to rust-fs-core v0.5.0.
