use std::{
    borrow::Cow,
    fmt::{self, Debug, Display, LowerHex, UpperHex},
    num::{NonZeroU64, ParseIntError},
    str::FromStr,
    sync::atomic::{AtomicU16, Ordering},
    time::SystemTime,
};

use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};

/// 64-bit id value.
/// FFI-safe.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct AssetId(NonZeroU64);

impl Serialize for AssetId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use std::io::Write;

        if serializer.is_human_readable() {
            let mut hex = [0u8; 16];
            write!(std::io::Cursor::new(&mut hex[..]), "{:x}", self.0).expect("Must fit");
            let hex = std::str::from_utf8(&hex).expect("Must be UTF-8");
            serializer.serialize_str(hex)
        } else {
            serializer.serialize_u64(self.0.get())
        }
    }
}

impl<'de> Deserialize<'de> for AssetId {
    fn deserialize<D>(deserializer: D) -> Result<AssetId, D::Error>
    where
        D: Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            let hex = Cow::<str>::deserialize(deserializer)?;
            hex.parse().map_err(Error::custom)
        } else {
            let value = NonZeroU64::deserialize(deserializer)?;
            Ok(AssetId(value))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ParseAssetIdError {
    #[error(transparent)]
    ParseIntError(#[from] ParseIntError),

    #[error("AssetId cannot be zero")]
    ZeroId,
}

impl FromStr for AssetId {
    type Err = ParseAssetIdError;
    fn from_str(s: &str) -> Result<Self, ParseAssetIdError> {
        let value = u64::from_str_radix(s, 16)?;
        match NonZeroU64::new(value) {
            None => Err(ParseAssetIdError::ZeroId),
            Some(value) => Ok(AssetId(value)),
        }
    }
}

#[derive(Debug)]
pub struct ZeroIDError;

impl AssetId {
    pub fn new(value: u64) -> Option<Self> {
        NonZeroU64::new(value).map(AssetId)
    }

    pub fn value(&self) -> NonZeroU64 {
        self.0
    }
}

impl TryFrom<u64> for AssetId {
    type Error = ZeroIDError;

    fn try_from(value: u64) -> Result<Self, ZeroIDError> {
        match NonZeroU64::try_from(value) {
            Ok(value) => Ok(AssetId(value)),
            Err(_) => Err(ZeroIDError),
        }
    }
}

impl Debug for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        LowerHex::fmt(&self.0.get(), f)
    }
}

impl UpperHex for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        UpperHex::fmt(&self.0.get(), f)
    }
}

impl LowerHex for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        LowerHex::fmt(&self.0.get(), f)
    }
}

impl Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        LowerHex::fmt(&self.0.get(), f)
    }
}

/// Context to generate IDs.
/// Each context guarantees to generate unique IDs.
/// Context initialized with different salt are guaranteed to generate different IDs.
pub struct AssetIdContext {
    /// Start of the epoch.
    epoch: SystemTime,

    /// 10 bits of the node id.
    node: u16,

    /// Counter first 12 bits of which are used for ID generation.
    counter: AtomicU16,
}

impl AssetIdContext {
    pub const fn new(epoch: SystemTime, node: u16) -> Self {
        AssetIdContext {
            epoch,
            node: node & 0x3FF,
            counter: AtomicU16::new(0),
        }
    }

    /// Generate new ID.
    pub fn generate(&self) -> AssetId {
        let timestamp = SystemTime::now()
            .duration_since(self.epoch)
            .expect("Epoch must be in relatively distant past")
            .as_millis() as u64;

        let node = self.node as u64;

        let mut counter = self.counter.fetch_add(1, Ordering::Relaxed) as u64;

        while counter & 0xFFF == 0 {
            // Single loop is expected once per 4096 id generated.
            // Multiple loop is highly improbable
            counter = self.counter.fetch_add(1, Ordering::Relaxed) as u64;
        }

        let id = (timestamp << 22) | (node << 12) | (counter & 0xFFF);
        let id = NonZeroU64::new(id.wrapping_mul(ID_MUL)).expect("Zero id cannot be generated");
        AssetId(id)
    }
}

/// GCD of values with least significant bit set and 2^N is always 1.
/// Meaning that multiplying any value with this constant is reversible
/// and thus does not break uniqueness.
const ID_MUL: u64 = 0xF89A4B715E26C30D;

#[allow(unused)]
const GUARANTEE_LEAST_SIGNIFICANT_BIT_OF_ID_MUL_IS_SET: [(); (ID_MUL & 1) as usize] = [()];
