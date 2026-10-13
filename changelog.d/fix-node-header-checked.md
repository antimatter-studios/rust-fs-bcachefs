### Fixed

- **A btree node is checked against the pointer that leads to it.** The
  reader believed what a node's header said about itself, so a node naming
  another btree or another level, or starting or ending somewhere its
  pointer does not, fed its keys into the wrong place. Such a node is now
  refused, as is a key format whose fields unpack past their width and a
  bset whose keys are out of order, both of which wrapped or re-sorted keys
  silently (#162).
