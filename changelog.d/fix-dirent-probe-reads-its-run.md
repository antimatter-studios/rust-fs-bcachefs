### Fixed

- **A create, link, rename or removal reads only its name's run of dirent
  hash slots.** Finding a name's slot read every dirents key of the directory
  from the name's hash on, so in a large directory one create could read
  most of its dirents nodes; it now stops at the first offset its run
  skips (#191).
