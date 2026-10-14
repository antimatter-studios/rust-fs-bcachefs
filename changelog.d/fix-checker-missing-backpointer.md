### Fixed

- **The checker reports a data extent with no backpointer (#163).** Each
  data extent written whole, not unwritten and not a piece of a checksummed
  region, must have its backpointer at its device sector, as the reference
  checker requires ("missing backpointer"); one without is reported as
  `missing_backpointer`, naming the extent.
