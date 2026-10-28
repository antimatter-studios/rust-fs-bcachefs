### Fixed

- The checker reports `nr_inodes`, `replicas` and `dev_data_type` accounting keys the reference refuses on read, where it called the image clean: a nonzero position byte after the kind's fields (`accounting_key_junk_at_end`), a replicas entry with no devices, more copies required than devices or devices not in increasing order (`accounting_replicas_bad`), and a value of more than three counters, or of none for `nr_inodes` and `replicas` (`accounting_key_counters_bad`).
