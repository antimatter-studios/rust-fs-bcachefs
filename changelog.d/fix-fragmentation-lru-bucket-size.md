### Fixed
- Equal-sequence superblock copies are selected in read order, with the later valid copy winning, avoiding false fragmentation LRU findings after a primary-only bucket-size change (#247).
