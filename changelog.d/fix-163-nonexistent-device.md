### Fixed

- **The checker reports a key on a device the filesystem does not have
  (#163).** An alloc or bucket_gens key filed under another device of a
  one-device filesystem was reported clean, while the reference checker
  stops on it ("pointer to nonexistent device"). It is now reported as
  `pointer_to_nonexistent_device`.
