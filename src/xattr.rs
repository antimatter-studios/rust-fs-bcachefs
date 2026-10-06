//! Extended attributes: the values of the xattrs btree.
//!
//! Provenance (docs/clean-room.md): the xattrs btree is documented (S1,
//! 11.3) as holding extended attributes, snapshot-aware. Each attribute is
//! one key of type `xattr` (11, S1's key type order) at
//! `inode:hash:snapshot`. The value layout was found by hexdump of the aged
//! fixture against the reference lister's `name:value` output and the
//! mount's own listing (S3, S4, S8):
//!
//! ```text
//! namespace u8, name length u8, value length le16, name (without its
//! namespace prefix), value, zero padding to a whole u64
//! ```

use crate::bkey::{key_type, Bkey};
use crate::error::{Error, Result};

/// One extended attribute: its full name, namespace prefix included
/// (`user.greeting`), and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Xattr {
    pub name: Vec<u8>,
    pub value: Vec<u8>,
}

/// Namespace numbers and the prefix a name carries in each. INFERRED from
/// the aged fixture: `user.*` attributes store 0 and `trusted.*` store 3.
/// Others (security, the POSIX ACLs) have not been seen and are refused
/// rather than guessed.
const NAMESPACES: &[(u8, &str)] = &[(0, "user."), (3, "trusted.")];

impl Xattr {
    pub fn from_key(k: &Bkey) -> Result<Self> {
        if k.key_type != key_type::XATTR {
            return Err(Error::Unsupported(format!("xattr key type {}", k.key_type)));
        }
        let v = &k.value;
        if v.len() < 4 {
            return Err(Error::Corrupt("xattr value shorter than its header".into()));
        }
        let (ns, name_len, val_len) = (
            v[0],
            v[1] as usize,
            u16::from_le_bytes([v[2], v[3]]) as usize,
        );
        let end = 4 + name_len + val_len;
        if end > v.len() {
            return Err(Error::Corrupt(format!(
                "xattr of {name_len}+{val_len} bytes in a {}-byte value",
                v.len()
            )));
        }
        let prefix = NAMESPACES
            .iter()
            .find(|(n, _)| *n == ns)
            .map(|(_, p)| *p)
            .ok_or_else(|| Error::Unsupported(format!("xattr namespace {ns}")))?;
        let mut name = prefix.as_bytes().to_vec();
        name.extend_from_slice(&v[4..4 + name_len]);
        Ok(Xattr {
            name,
            value: v[4 + name_len..end].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bkey::Bpos;

    fn key(value: Vec<u8>) -> Bkey {
        Bkey {
            key_type: key_type::XATTR,
            version_hi: 0,
            version_lo: 0,
            size: 0,
            pos: Bpos::default(),
            value,
        }
    }

    #[test]
    fn a_user_attribute_decodes_with_its_prefix() {
        // Bytes as the aged fixture stores user.greeting = "hello".
        let mut v = vec![0, 8, 5, 0];
        v.extend_from_slice(b"greetinghello");
        v.resize(24, 0);
        let x = Xattr::from_key(&key(v)).unwrap();
        assert_eq!(x.name, b"user.greeting");
        assert_eq!(x.value, b"hello");
    }

    #[test]
    fn a_length_past_the_value_is_corrupt_and_an_unknown_namespace_is_refused() {
        assert!(matches!(
            Xattr::from_key(&key(vec![0, 8, 200, 0, 0, 0, 0, 0])),
            Err(Error::Corrupt(_))
        ));
        assert!(matches!(
            Xattr::from_key(&key(vec![9, 1, 1, 0, b'a', b'b', 0, 0])),
            Err(Error::Unsupported(_))
        ));
    }
}
