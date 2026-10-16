### Fixed

- **Oracle image copies no longer fill the disk (#235).** A test that
  copied a fixture image left the copy behind in a PID-named temp directory.
  Each mutable copy now retains its directory owner until the caller finishes,
  and the test runner removes its private scratch root on success, failure,
  abort or termination. The next run recovers marked roots of dead runs.
