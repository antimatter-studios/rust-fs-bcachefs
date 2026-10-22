### Fixed

- **A sparse file keeps its holes when it is written inside or cut short
  (#156).** `Writer::write_at` and `Writer::truncate` rewrite a file whole,
  and they read its holes as zeros and stored those zeros back, so one write
  anywhere in a sparse file allocated every hole it had. Now only the blocks
  that held something, and the blocks a write covers, are rewritten; the
  holes have no extent and still read as zeros.
