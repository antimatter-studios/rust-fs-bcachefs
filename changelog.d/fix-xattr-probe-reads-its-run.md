### Fixed

- **Setting an xattr reads its run of slots, not the rest of the table.**
  The probe for an xattr's slot read every xattr key of the inode from the
  name's hash onwards, so on an inode with many xattrs one set read most of
  its table; it now stops at the first slot its run skips (#193).
