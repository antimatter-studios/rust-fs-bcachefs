### Fixed

- **The last ChaCha20 counter block can be read (#318).** A valid request
  ending in block `0xffff_ffff` no longer fails with an out-of-range error.
  Requests extending beyond the 32-bit counter are still refused before
  changing the input.
