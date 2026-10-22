### Added

- **Encrypted extents with 128-bit MACs are read (#335).** A filesystem
  formatted with `--wide_macs` now reads: each data extent's full 128-bit
  Poly1305 MAC, the crc entry's low then high checksum word, is verified
  before the extent is decrypted, and a mismatch in either half is refused
  as a bad checksum naming the MAC.
