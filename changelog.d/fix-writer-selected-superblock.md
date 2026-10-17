### Fixed

- **Writes preserve the selected superblock copy (#250).** When a newer
  backup is authoritative, or the primary is unreadable, the writer uses
  that backup's bytes for root updates and every rewritten copy instead of
  reverting to the primary's metadata.
