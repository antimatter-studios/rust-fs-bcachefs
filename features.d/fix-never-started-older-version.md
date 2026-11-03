## Checking

| Images formatted and never started | Supported: an image at a version the formatter leaves to its first start is clean unless the superblock's header, layout, options or member say what the reference reports, under `invalid_sb_*` and `device_size_too_small`. Superblock bit 0 clear with a `journal` or `clean` field is read as the started filesystem it records, and reported where the reference starts it anew. An encrypted one is still refused | Unreleased (#532) | #505 | `tests/check_oracle_never_started.rs`, `src/check/never_started_tests.rs` |
