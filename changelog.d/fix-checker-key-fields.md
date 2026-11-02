### Fixed
- The checker reports a snapshot whose reserved pad word is nonzero (`snapshot_pad_nonzero`) and a subvolume whose state is unset (`subvol_state_bad`).
- A dirent's type is read from the low five bits of its type byte, so the checker no longer reports entries the reference accepts, and readers take bits 5 and 6 as the reference does.
