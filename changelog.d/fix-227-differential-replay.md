### Fixed

- **The three pinned differential disagreements are replayed and asserted
  (#227).** A `replay` dispatch of the differential workflow judges the
  recorded fixture run, keeps only the named cases the reference judged
  without crashing (mutation patch, mutation and verdict, the reference's
  whole output), uploads them, and fails unless each lands in its exact
  class. No case is kept by default.
