### Fixed

- **The checker reports a backpointer its extent does not point at (#433).**
  Where a bucket's backpointers do not add up to its dirty sectors, a backpointer
  naming an extent with no pointer at its sector is `backpointer_to_missing_ptr`,
  as the reference checker reports it.
