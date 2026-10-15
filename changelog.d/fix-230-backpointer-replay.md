### Fixed

- **The #230 missing-backpointer case has a pinned replay with the
  reference's own listings.** `fuzz/differential-replay-230.tsv` pins fixture
  run 38680345154 and case `default-55`; a `replay` dispatch with
  `replay_file` set to it keeps the reference's backpointers and extents
  listings and raw `fsck -n` output of the fixture unchanged and mutated, and
  fails unless the case lands in its class and the fixture's unchanged control
  is clean. Clean means exit 0 with nothing printed beyond a clean image's
  lines, since the reference exits 0 under `-n` after a repair. Listings of
  a crash, hang or internal fault are kept as a status only.
