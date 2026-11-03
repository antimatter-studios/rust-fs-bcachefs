### Fixed
- The checker reports a snapshot's state word and a subvolume's state and pad words as the reference does (`snapshot_state_bad`, `snapshot_state_bitflip`, `subvol_state_bad`, `subvol_state_bitflip`, `subvol_state_invalid`, `subvol_pad_nonzero`).
- The checker reports a snapshot in a state at odds with its subvolume, its children or the keys accounted to it (`snapshot_deleted_but_subvol_live`, `snapshot_deleted_has_live_children`, `snapshot_deleted_but_has_data`).
- The checker no longer reports an unset subvolume state below format version 1.39, where the reference has no finding for it.
