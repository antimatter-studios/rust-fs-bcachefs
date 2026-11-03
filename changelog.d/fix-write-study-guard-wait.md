### Fixed

- Write-study cleanup no longer waits again for an interrupted tool and bounds FUSE termination before unmounting. Its lifecycle guard shares progress-aware waits and signal reset helpers with the fixture guard, tolerates slow tools and inherited ignored signals, and diagnoses stalled or leaking supervisors by case.
