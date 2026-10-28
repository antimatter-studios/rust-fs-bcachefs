### Fixed

- The checker reports a backpointer at a nonzero snapshot as `bkey_snapshot_nonzero`, and one whose level is above 4 as `backpointer_level_bad` (#219), where it called the image clean.
