//! Durable read positions distinguish histories branched from the same backup.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

/// A branch identifier assigned to a standalone backup or offline restore.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadIncarnation([u8; 16]);

impl Display for ReadIncarnation {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for ReadIncarnation {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (digits, remainder) = value.as_bytes().as_chunks::<2>();
        let mut bytes = [0; 16];
        if digits.len() != bytes.len() || !remainder.is_empty() {
            return Err("invalid history read incarnation");
        }
        for (byte, &[hi, lo]) in bytes.iter_mut().zip(digits) {
            *byte = (hex(hi)? << 4) | hex(lo)?;
        }
        Ok(Self(bytes))
    }
}

fn hex(digit: u8) -> Result<u8, &'static str> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        _ => Err("invalid history read incarnation"),
    }
}

impl Serialize for ReadIncarnation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ReadIncarnation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

/// The public ID for a durable history snapshot. Its watermark keeps the
/// existing receipt and retention ordering within one read incarnation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReadSnapshot {
    watermark: u64,
    incarnation: Option<ReadIncarnation>,
}

impl ReadSnapshot {
    pub const fn new(watermark: u64, incarnation: Option<ReadIncarnation>) -> Self {
        Self {
            watermark,
            incarnation,
        }
    }

    pub const fn watermark(self) -> u64 {
        self.watermark
    }

    pub const fn incarnation(self) -> Option<ReadIncarnation> {
        self.incarnation
    }
}

impl From<u64> for ReadSnapshot {
    fn from(watermark: u64) -> Self {
        Self::new(watermark, None)
    }
}

impl Display for ReadSnapshot {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if let Some(incarnation) = self.incarnation {
            write!(f, "r1:{incarnation}:")?;
        }
        self.watermark.fmt(f)
    }
}

impl FromStr for ReadSnapshot {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (incarnation, watermark) = match value.strip_prefix("r1:") {
            Some(tagged) => {
                let (incarnation, watermark) = tagged
                    .split_once(':')
                    .ok_or("invalid history snapshot ID")?;
                (Some(incarnation.parse()?), watermark)
            }
            None => (None, value),
        };
        if !watermark.bytes().all(|digit| digit.is_ascii_digit()) {
            return Err("invalid history snapshot ID");
        }
        Ok(Self::new(
            watermark
                .parse()
                .map_err(|_| "invalid history snapshot ID")?,
            incarnation,
        ))
    }
}

impl Serialize for ReadSnapshot {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.incarnation {
            Some(_) => serializer.collect_str(self),
            None => serializer.serialize_u64(self.watermark),
        }
    }
}

impl<'de> Deserialize<'de> for ReadSnapshot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Numeric(u64),
            Text(String),
        }

        match Wire::deserialize(deserializer)? {
            Wire::Numeric(watermark) => Ok(watermark.into()),
            Wire::Text(value) => value.parse().map_err(serde::de::Error::custom),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_preserve_legacy_numbers_and_round_trip_fenced_tokens() {
        let legacy = ReadSnapshot::from(42);
        assert_eq!("42".parse::<ReadSnapshot>().unwrap(), legacy);
        assert_eq!(legacy.to_string(), "42");

        let token = "r1:5416f2c714f6405fb999c5f4d5f9ee2b:42";
        let snapshot = token.parse::<ReadSnapshot>().unwrap();
        assert_eq!(snapshot.watermark(), legacy.watermark());
        assert!(snapshot.incarnation().is_some());
        assert_ne!(snapshot, legacy);
        assert_eq!(snapshot.to_string(), token);
        for malformed in [
            "r1:",
            "r1:abcd:1",
            "r1:5416f2c714f6405fb999c5f4d5f9ee2b:-1",
            "0:42",
        ] {
            assert!(malformed.parse::<ReadSnapshot>().is_err());
        }
    }
}
