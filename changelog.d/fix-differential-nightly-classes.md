### Fixed

- The checker reads the btree of a per-snapshot accounting counter as the 32-bit field it is, and reports `accounting_mismatch` for a counter whose btree id names no btree, as the reference does for "btree=(unknown btree 65536)". It used to read one byte of the field and compare the counter with another btree's keys.
