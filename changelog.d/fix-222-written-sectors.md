### Fixed

- **A torn btree node is refused (#222).** A node whose valid bsets end
  before the sectors its parent's pointer counts as written -- a later bset
  missing, cut short or carrying another node's sequence -- was read as the
  surviving prefix and reported clean. The reader and the checker now refuse
  it as corrupt, as the reference checker does.
