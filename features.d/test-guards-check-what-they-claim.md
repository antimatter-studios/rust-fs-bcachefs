## Writing (`--features write`)

| A write or import refused for space exits 4 in the CLI, the volume's state refusing it, with the free-bucket shortfall in the error | Experimental | Unreleased (#484) | #456 | `write_oracle.rs`, `tests/cli/test-import.sh` |
| An import whose one journal entry does not fit a journal bucket, refused although the file fits the free space | Refused | Unreleased (#484) | #485 | `write_oracle.rs` (the out-of-space test fills the image first because of this limit) |
| A journalled import that fails part way through a sequence can leave the image failing the checker | Experimental | Unreleased (#484) | #486 | none yet |
