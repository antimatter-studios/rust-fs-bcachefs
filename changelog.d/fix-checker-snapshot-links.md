### Fixed

- **The checker reports damaged snapshot links.** A snapshot tree whose
  master subvolume is missing, a snapshot naming a tree that is missing or
  whose root is not the root its parents lead to, a parent cycle, and a
  skiplist entry naming neither the snapshot nor one of its ancestors are
  reported rather than passed as clean.
