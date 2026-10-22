### Fixed

- **Unpacked keys' versions are read as the reference lister prints them.**
  The 12 version bytes are read as version_lo (u64) then version_hi (u32);
  they were read the other way round, so every unpacked extent key of an
  encrypted, compressed image reported version 0 and its extent nonce was
  wrong. Keys the writer makes keep the bytes they had.
