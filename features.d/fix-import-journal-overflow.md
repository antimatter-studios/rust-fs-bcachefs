## Writing (`--features write`)

rename: Journal reclaim: a full journal is written into the nodes in place and the session goes on
| Journal reclaim: a journal three quarters full is written into new nodes in free buckets, one superblock write ends it, and the session goes on; a reclaim that stops part way leaves the journal to replay | Experimental | Unreleased (#489) | refused when a continued journal has no clean field or holds interior node keys (#113), or when too few buckets are free for its nodes | `oracle_write_local.rs`, `write_oracle.rs` |
| Importing a file larger than one journal entry holds, in runs that each fit one, and removing a staged file of any size when an import fails | Experimental | Unreleased (#489) | | `write_oracle.rs` |
| A commit larger than one journal bucket: `rm --journal`, `import --replace` over a large file and `put --journal` are refused before any metadata is written | Refused | Unreleased (#489) | #494 | `write_oracle.rs` |
