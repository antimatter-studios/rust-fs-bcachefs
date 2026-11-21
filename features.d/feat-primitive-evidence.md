## Interfaces

| Primitive evidence: every superblock copy with its own verdict, the layout at sector 7, the copy a reader chooses and where valid copies disagree (`primitive::survey_superblocks`) | Supported | Unreleased | | `oracle_primitive.rs` |
| Primitive evidence: a btree node read at any offset, every bset's checksum verdict, and every key that still decodes beside the damage (`primitive::inspect_node`) | Supported | Unreleased | | `oracle_primitive.rs` |
| Primitive evidence: every btree node on the device found by its magic, older versions the live tree no longer reaches included, resumable (`primitive::scan_nodes`), and the nodes the live tree reaches with where that walk breaks (`primitive::live_nodes`) | Supported | Unreleased | | `oracle_primitive.rs` |
| Primitive evidence: every journal entry in the journal buckets, a torn entry listed with its verdict and its bucket read past it, and the roots each entry records (`primitive::survey_journal`) | Supported | Unreleased | | `oracle_primitive.rs` |
