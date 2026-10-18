### Fixed

- **Every `fs.bcachefs` verb uses the filesystem device offset.** Global
  `--offset BYTES` selects the filesystem inside an image or device for reads,
  experimental writes and read-only preflight checks. Malformed offsets
  return the shared exit-2 error.
