### Fixed

- **A reflinked file can be written (#169).** `write_at`, `append` and
  `truncate` refused a file whose data is shared through a `reflink_p`
  ("extent key type 15 cannot be rewritten"), so a cloned file could not be
  modified at all. The rewrite now reads the shared data and copies it into
  extents of the file's own, dropping one reference to each indirect
  extent; the last reference frees it, as an unlink does.
