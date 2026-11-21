### Fixed

- Positioned writes retain unaffected file extents and write only the covered range; journalled replacements and removals that exceed one entry refuse before changing the visible file.
- Journal reclaim records a btree root created during the current session, so repeated imports continue after the journal fills.
