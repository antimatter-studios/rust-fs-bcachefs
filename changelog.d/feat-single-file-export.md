### Added

- `fs.bcachefs TARGET export PATH DEST [--replace]` copies a regular file through the shared bounded transfer, reporting its byte count and SHA-256. Local destinations appear only after a complete copy; existing names require `--replace`, and `DEST -` writes raw bytes to stdout. Failed reads preserve existing destinations and remove staging files.
