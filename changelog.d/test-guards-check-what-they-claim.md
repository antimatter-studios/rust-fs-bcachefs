### Fixed

- **A write refused for space is the volume's state refusing it.** The write
  verbs and `import` exit 4 with the writer's own words when too few buckets
  are free, not exit 3 with "not implemented".
