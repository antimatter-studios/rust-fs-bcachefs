### Fixed

- The checker reports a size on a `set` key in reconcile_work_phys, a snapshot on a compression or per-snapshot accounting key, and a value on a `set` key in accounting, as the reference checker does.
- The checker no longer reports a key of type 50 in the accounting btree, which the reference checker accepts.
