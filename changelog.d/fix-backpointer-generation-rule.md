### Fixed

- The checker no longer reports a data backpointer's generation in a bucket that has no backpointer at its pointer's generation. The reference checker passes such a bucket, which an earlier version of this crate's writer could leave. A generation beside a backpointer at the pointer's generation is still reported.
