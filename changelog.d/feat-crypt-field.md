### Changed

- **An encrypted filesystem's refusal says whether a passphrase seals its
  key (#270).** The superblock's crypt field is read, and opening an
  encrypted filesystem is refused naming its master key as sealed by a
  passphrase or stored unsealed (formatted without one). Encrypted
  filesystems are still not read (#159).
