### Fixed

- **The checker reports a btree-node backpointer displaced from every reachable node's physical address (#453).** The check requires a reachable target and preserves the reference-accepted header mutations at real node addresses. Missing backpointers are not inferred from this check.
