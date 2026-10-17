### Fixed

- **The superblock-copy check reads optional fields only from the copy it
  selects.** The checker validates optional superblock fields only in the
  checksum-valid copy chosen by sequence number, as the reference checker
  does. Malformed fields in an unused copy no longer produce a false
  superblock-copy finding, and malformed fields in the selected copy are
  reported instead of falling back to an older copy. Unreadable headers and
  bad checksums are still reported.
