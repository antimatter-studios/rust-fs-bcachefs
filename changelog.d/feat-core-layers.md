### Added

- **The volume, primitive and file layers on rust-fs-core 0.5's shared
  traits.** `volume::Volume` opens strictly for the file API or for salvage
  alone, and implements `VolumeAPI`, `OpenVolume`, `PrimitiveAPI` and
  `FileAPI`; `salvage::adapter::BcachefsSalvage` implements
  `SalvageAdapter`, decoding compressed extents through `read_encoded`. They
  pass rust-fs-core's conformance helpers on the reference formatter's
  fixtures, every file rebuilt from its candidate's walk byte for byte, on a
  damaged copy, and in the salvage conformance set over an image the kernel
  module wrote and deleted files in. The existing `Filesystem`, `primitive`
  and `salvage` APIs are unchanged.
