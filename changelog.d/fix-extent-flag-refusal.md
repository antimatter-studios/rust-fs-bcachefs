### Fixed

- Extent pointer flag values 2 and 6 no longer produce a cached-pointer refusal when the reference checker accepts them; combined unwritten flags still read as zeros. Cached pointers, stale generations and missing allocation keys remain refused.
