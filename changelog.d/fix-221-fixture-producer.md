### Fixed

- **The differential run takes its fixtures from a run that produced them
  (#221).** The automatic choice no longer assumes the newest green main run
  carries fixtures; it walks the last 30 and takes the first with an unexpired
  `fixtures` artifact, failing by name when none has one. An explicit
  `fixture_run` still needs its digest and still fails on a mismatch.
