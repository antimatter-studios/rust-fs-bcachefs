### Added

- **Recursive listings have finite bounds.** `fs.bcachefs ls --recursive`
  (`-R`), also spelled `tree`, walks in byte-sorted depth-first order with
  default depth 64 and 100000 entries. Exact fits succeed; a stopped walk
  retains its report and names the bound or unreadable path with exit 1.
  Symlinks are never followed; subvolume and snapshot roots are marked and
  entered, and repeated inode/snapshot identities are marked and not entered.
