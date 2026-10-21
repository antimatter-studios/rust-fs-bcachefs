### Fixed

- **The checker reports a snapshot tree whose master subvolume is in
  another tree.** A snapshot tree naming a subvolume that exists but whose
  snapshot belongs to a different tree is reported rather than passed as
  clean.
