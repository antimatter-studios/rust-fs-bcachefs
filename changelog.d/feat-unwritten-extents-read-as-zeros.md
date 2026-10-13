### Added

- **Preallocated space reads as zeros (#158).** An extent whose pointer is
  marked unwritten, which the reference kernel module makes when a MiB is
  preallocated with `fallocate` in a `nocow` file, used to make the whole
  file unreadable; it now reads as zeros over its range, as the reference
  reads it, and the checker accepts it. The flag was observed rather than
  guessed: the kernel oracle builds a fifth image for it, and
  `tests/oracle_unwritten.rs` reads both a `nocow` and an ordinary
  preallocated file back as their 4 KiB in a MiB of zeros. A pointer with
  any other flag (S1's cached pointers, never observed) is still refused.
