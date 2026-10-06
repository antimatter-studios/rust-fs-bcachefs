//! Extended attributes: the values of the xattrs btree.

/// One extended attribute: its full name, namespace prefix included
/// (`user.greeting`), and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Xattr {
    pub name: Vec<u8>,
    pub value: Vec<u8>,
}
