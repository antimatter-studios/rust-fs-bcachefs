### Added

- **Incompressible encrypted extents are read (#356).** On an encrypted
  filesystem formatted with compression, data the writer found
  incompressible is stored with the incompressible compression type; such
  an extent is now authenticated and decrypted with the same nonce an
  uncompressed extent uses, instead of being refused. Compressed (LZ4)
  encrypted extents are still refused (#330).
- **Partly overwritten encrypted extents whose checksum was narrowed are
  read (#331).** When the reference shrinks such an extent's crc entry to
  its live sectors, the entry records how many sectors in they start; the
  keystream and the MAC are now taken from that point instead of the
  extent being refused.
