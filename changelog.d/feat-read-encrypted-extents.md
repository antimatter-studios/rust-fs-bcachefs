### Added

- **An encrypted filesystem whose master key is stored unsealed is read
  (#272).** Formatted with `--no_passphrase`, it now opens, and every file
  reads: each data extent is authenticated with the 80-bit Poly1305 MAC in
  its crc entry, then decrypted with ChaCha20 under the master key with a
  nonce made from its key's version. An extent whose MAC does not verify is
  refused as a bad checksum naming the MAC. Compressed and partly
  overwritten encrypted extents are refused rather than read (#275), and a
  filesystem whose key a passphrase seals is still refused (#273).
