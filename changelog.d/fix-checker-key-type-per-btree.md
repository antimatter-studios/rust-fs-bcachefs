### Fixed

- **The checker reports a key of a type its btree does not hold.** A key of
  a type the reference checker refuses in the `logged_ops`, `accounting` or
  `reconcile_work_phys` btree is a finding, as the reference checker's
  `invalid bkey` is, rather than passing unchecked.
- **Accepted key types match the recorded reference verdicts.** The three
  judged btrees accept `set` and unnamed type 40, and findings for
  `reconcile_work_phys` identify the tree by name.
