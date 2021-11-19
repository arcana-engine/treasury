use std::{
    borrow::{Borrow, Cow},
    fmt::{self, Debug, LowerHex, UpperHex},
    num::ParseIntError,
    ops::Deref,
    path::Path,
    str::FromStr,
};

use serde::{
    de::{Deserialize, Deserializer, Error},
    ser::Serializer,
    Serialize,
};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HashSha256 {
    bytes: [u8; 32],
}

impl Deref for HashSha256 {
    type Target = [u8; 32];
    fn deref(&self) -> &[u8; 32] {
        &self.bytes
    }
}

impl Borrow<[u8; 32]> for HashSha256 {
    fn borrow(&self) -> &[u8; 32] {
        &self.bytes
    }
}

impl Borrow<[u8]> for HashSha256 {
    fn borrow(&self) -> &[u8] {
        &self.bytes
    }
}

impl Debug for HashSha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        LowerHex::fmt(self, f)
    }
}

impl LowerHex for HashSha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            f.write_str("0x")?;
        }

        for chunk in self.bytes.chunks(16) {
            match chunk.len() {
                0..=15 => {
                    for byte in chunk {
                        write!(f, "{:x}", byte)?;
                    }
                }
                16 => {
                    let v = u128::from_be_bytes(chunk.try_into().unwrap());
                    write!(f, "{:x}", v)?;
                }
                _ => unreachable!(),
            }
        }
        Ok(())
    }
}

impl UpperHex for HashSha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            f.write_str("0X")?;
        }

        for chunk in self.bytes.chunks(16) {
            match chunk.len() {
                0..=15 => {
                    for byte in chunk {
                        write!(f, "{:X}", byte)?;
                    }
                }
                16 => {
                    let v = u128::from_be_bytes(chunk.try_into().unwrap());
                    write!(f, "{:X}", v)?;
                }
                _ => unreachable!(),
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ParseHashError {
    #[error(transparent)]
    ParseIntError(#[from] ParseIntError),

    #[error("Failed to parse hex hash value. Not enough digits")]
    NotEnoughDigits,
}

impl FromStr for HashSha256 {
    type Err = ParseHashError;
    fn from_str(mut s: &str) -> Result<Self, ParseHashError> {
        let mut bytes = [0; 32];
        for chunk in bytes.chunks_mut(16) {
            let value =
                u128::from_str_radix(s.get(0..32).ok_or(ParseHashError::NotEnoughDigits)?, 16)?;
            s = &s[32..];
            chunk.copy_from_slice(&value.to_be_bytes());
        }
        Ok(HashSha256 { bytes })
    }
}

impl HashSha256 {
    pub fn new(data: impl AsRef<[u8]>) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash = hasher.finalize();
        let mut bytes = [0; 32];
        bytes.copy_from_slice(&hash);
        HashSha256 { bytes }
    }

    pub fn file_hash(path: &Path) -> std::io::Result<HashSha256> {
        // Check for a duplicate.
        let mut hasher = Sha256::new();

        // let mut file = File::open(&path)?;
        // std::io::copy(&mut file, &mut hasher)?;

        let content = std::fs::read(&path)?;
        hasher.update(dbg!(&content));

        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&hasher.finalize());
        Ok(HashSha256 { bytes })
    }
}

impl Serialize for HashSha256 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use std::io::Write;

        if serializer.is_human_readable() {
            let mut hex = [0u8; 64];
            write!(std::io::Cursor::new(&mut hex[..]), "{:x}", self).expect("Must fit");
            let hex = std::str::from_utf8(&hex).expect("Must be UTF-8");
            serializer.serialize_str(hex)
        } else {
            serializer.serialize_bytes(&self.bytes)
        }
    }
}

impl<'de> Deserialize<'de> for HashSha256 {
    fn deserialize<D>(deserializer: D) -> Result<HashSha256, D::Error>
    where
        D: Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            let hex = Cow::<str>::deserialize(deserializer)?;
            hex.parse().map_err(Error::custom)
        } else {
            let bytes = serde_bytes::deserialize::<Cow<[u8]>, _>(deserializer)?;
            let bytes = TryFrom::try_from(bytes.as_ref()).map_err(Error::custom)?;
            Ok(HashSha256 { bytes })
        }
    }
}
