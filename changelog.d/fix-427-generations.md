### Fixed

- **The checker compares two more generations (#427, #471).** A user-data
  backpointer's generation against its pointer's, and an alloc key's generation
  when no `bucket_gens` key covers its bucket, which then counts as 0.
- **The writer records a data extent's generation in its backpointer (#427).**
  It wrote 0 whatever the bucket's generation, so a file in a reused bucket
  disagreed with its backpointer.
