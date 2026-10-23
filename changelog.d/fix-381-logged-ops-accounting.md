### Fixed

- **The checker reports a logged_ops accounting key out of step with the
  btree's nodes (#381).** The key holds the btree's node sectors, nodes and
  interior nodes. The reference reports a mismatch as `accounting_mismatch`,
  where this checker called the image clean. It now counts the logged_ops
  nodes (`btree::node_counts`) and reports a mismatch on a cleanly shut down
  filesystem.
