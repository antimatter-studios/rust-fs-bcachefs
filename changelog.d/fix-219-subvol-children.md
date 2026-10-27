### Fixed

- The checker reports a subvolume_children entry at `parent:child` whose child is no subvolume, or whose subvolume's `fs_parent` is another parent, as `subvol_children_entry_wrong` (#382), where it called the image clean.
