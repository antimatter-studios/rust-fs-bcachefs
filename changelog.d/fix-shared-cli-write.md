### Added

- **`fs.bcachefs write PATH` creates or replaces a regular file.** Input
  comes from stdin or `--from FILE`; `--mode` and `--journal` retain the
  experimental library writer's creation and commit options. `put` is an
  identical alias, including its canonical `write` report.
- Predictable capability, path and input refusals occur before the target
  is opened for writing and use the shared error statuses on stderr.
