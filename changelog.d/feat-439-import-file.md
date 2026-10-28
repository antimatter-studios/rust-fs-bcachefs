### Added

- `fs.bcachefs TARGET import [--replace] [--verify] SOURCE PATH` streams a local file or stdin into the image through the shared import: the bytes go to a staged file beside PATH, every commit through the journal, and are published by one rename, which replaces an existing PATH in the same transaction when `--replace` is given. `--verify` reads the staged file back through a fresh open of the image before publishing, and a failed import removes the staged file and leaves PATH as it was.
