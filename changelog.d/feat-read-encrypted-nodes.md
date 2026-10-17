### Added

- **An encrypted filesystem's btree nodes are verified and decrypted when
  its master key is stored unsealed (#271).** Each node's records are
  authenticated with their 128-bit Poly1305 MAC and decrypted with ChaCha20
  under the master key from the superblock's crypt field, so every btree of
  a filesystem formatted with `--no_passphrase` reads as the reference
  lister lists it. A node whose MAC does not verify is refused as a bad
  checksum naming the MAC. Opening such a filesystem to read files is still
  refused: its data extents are #272's (#159).
