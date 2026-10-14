### Fixed

- **A write to a file with preallocated space keeps that space reading as
  zeros (#207).** The writer rewrites a file whole from what it reads of
  it, and it read an unwritten extent's device sectors as the file's data,
  so any `write_at`, `append` or `truncate` on such a file stored whatever
  the device held there, a deleted file's bytes on a reused bucket. Those
  ranges are now zeros, as the reader reads them.
