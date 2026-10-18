### Added

- **Incompressible encrypted extents are read (#356).** On an encrypted
  filesystem formatted with compression, data the writer found
  incompressible is stored with the incompressible compression type; such
  an extent is now authenticated and decrypted with the same nonce an
  uncompressed extent uses, instead of being refused. Compressed (LZ4)
  encrypted extents are still refused (#330).
