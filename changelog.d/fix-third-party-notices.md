### Fixed

- **The CLI tarball and the static library carry their crates' licence
  notices.** Both link nineteen crates under MIT, Apache-2.0, Zlib and
  0BSD, and carried only this repository's `LICENSE`. They now ship
  `THIRD-PARTY-LICENSES.md` beside it, generated from `cargo metadata` by
  `scripts/third-party-notices.py`, and `chore lint` fails when a shipped
  crate has no notice there (#155).
