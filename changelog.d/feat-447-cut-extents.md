### Changed

- **A write into part of an extent cuts it instead of rewriting it
  (#447).** `Writer::write_at`, `append` and `truncate` read, freed and
  wrote again every extent a write overlapped, whole: a 10-byte write into
  a 64-sector extent rewrote all 64 sectors. An uncompressed, checksummed
  extent is now cut as the reference cuts it: its parts outside the write
  keep their data, pointer and checksum entry, with the entry's offset and
  the key's size naming each part, and only the written blocks are stored
  anew. A compressed, unchecksummed or encrypted extent is still rewritten
  whole.
