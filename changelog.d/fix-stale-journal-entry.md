### Fixed

- **A damaged journal entry outside the replay window no longer makes the
  filesystem unreadable.** The journal is a ring, and entries older than
  the window are stale writes recovery never reads, but the reader parsed
  every entry and refused the filesystem over one damaged anywhere. Damage
  older than the window, or newer than its last flush, is now passed over;
  damage inside the window is still refused (#165).
