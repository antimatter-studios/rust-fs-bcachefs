### Added

- An encrypted filesystem's journal entries are verified against their 128-bit MAC and decrypted when read, so the journal of an unclean encrypted image lists and replays as the reference lists it. An entry whose MAC does not verify is not read, and one carrying a plain checksum on an encrypted filesystem is refused.
