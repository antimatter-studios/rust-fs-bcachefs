### Fixed
- The checker reports a replicas accounting key holding sectors on a device the filesystem lacks (`insufficient_devices_to_start`, `accounting_to_invalid_device`), or not in the superblock's replicas when its data type must be (`accounting_replicas_not_marked`).
- The checker reports a replicas accounting key holding sectors of an unknown data type (`accounting_mismatch`).
- The checker reports a replicas accounting key requiring all of several devices (`accounting_replicas_bad`).
