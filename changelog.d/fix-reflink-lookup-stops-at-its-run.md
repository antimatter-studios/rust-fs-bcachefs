### Fixed

- **Unlinking a reflinked file reads its run of indirect extents, not the
  rest of the reflink btree (#193).** Dropping a `reflink_p`'s references
  collected every reflink key from the pointer's start to the end of the
  btree before it looked at any of them, so its cost grew with the reflink
  btree rather than with the file. It now reads through a cursor and stops
  at the run's end or at its first gap.
