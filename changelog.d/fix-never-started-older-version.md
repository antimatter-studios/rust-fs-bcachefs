### Fixed

- The checker recognises an image the reference formatter left to its first start, at a format version below the one it starts itself, and judges its superblock instead of reporting a `journal` problem. A superblock with damage the reference reports is reported under the reference's error names, and one that records a start is still read as a started filesystem.
