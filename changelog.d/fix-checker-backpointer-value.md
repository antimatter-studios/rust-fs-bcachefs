### Fixed

- The checker reports `backpointer_to_missing_ptr` when a written data extent's backpointer value disagrees with its target position, physical length, btree, level or data type at an already-supported backpointer location.
