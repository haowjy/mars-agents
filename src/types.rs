use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;

macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Serialize, Deserialize, Hash, Eq, PartialEq, Clone, Debug, Ord, PartialOrd,
        )]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl Deref for $name {
            type Target = str;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<$name> for String {
            fn eq(&self, other: &$name) -> bool {
                *self == other.0
            }
        }
    };
}

string_newtype!(SourceName);
string_newtype!(ItemName);
string_newtype!(CommitHash);
string_newtype!(ContentHash);

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Wrapper<T> {
        value: T,
    }

    #[test]
    fn source_name_roundtrip() {
        let v = Wrapper {
            value: SourceName::from("base"),
        };
        let s = toml::to_string(&v).unwrap();
        let out: Wrapper<SourceName> = toml::from_str(&s).unwrap();
        assert_eq!(v, out);
    }

    #[test]
    fn item_name_roundtrip() {
        let v = Wrapper {
            value: ItemName::from("coder"),
        };
        let s = toml::to_string(&v).unwrap();
        let out: Wrapper<ItemName> = toml::from_str(&s).unwrap();
        assert_eq!(v, out);
    }

    #[test]
    fn commit_hash_roundtrip() {
        let v = Wrapper {
            value: CommitHash::from("abc123"),
        };
        let s = toml::to_string(&v).unwrap();
        let out: Wrapper<CommitHash> = toml::from_str(&s).unwrap();
        assert_eq!(v, out);
    }

    #[test]
    fn content_hash_roundtrip() {
        let v = Wrapper {
            value: ContentHash::from("sha256:abc123"),
        };
        let s = toml::to_string(&v).unwrap();
        let out: Wrapper<ContentHash> = toml::from_str(&s).unwrap();
        assert_eq!(v, out);
    }
}
