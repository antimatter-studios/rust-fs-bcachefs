### Fixed

- The checker reports a journal entry whose declared length exceeds its bucket, including stale entries outside the replay window, instead of silently discarding it. This matches the reference checker's `jset_past_bucket_end` verdict for the pinned aged-unclean differential case.
