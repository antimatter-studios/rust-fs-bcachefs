### Fixed

- **The checker reports a backpointer on a device the filesystem does not
  have (#383).** A backpointer's position names its device. One moved to
  another device was called clean, while the reference checker reports
  "backpointer for missing device". It is now reported as
  `backpointer_for_missing_device`.
