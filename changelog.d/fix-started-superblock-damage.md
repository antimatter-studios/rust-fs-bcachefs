### Fixed

- The checker reports the superblock damage the reference refuses on a started filesystem, which it used to call clean: a bad `block_size`, `version` or minimum, `offset`, user UUID, `time_precision`, unknown feature bit, layout, member or option out of range. The problem is named under the reference's `invalid_sb_*` errors, and nothing past such a superblock is read.
