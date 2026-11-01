### Fixed

- The checker judges an inode's has_inode_opts flag by the reference's rule
  (#379): `project`, `nocow` and `inodes_32bit` count as options, and
  `casefold` counts on a directory only.
