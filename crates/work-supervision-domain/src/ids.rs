use std::fmt;

use crate::Refusal;

fn is_lower_hex(text: &str) -> bool {
    text.bytes()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn hex_of(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        for nibble in [byte >> 4, byte & 0x0f] {
            if let Some(character) = HEX.get(usize::from(nibble)) {
                text.push(char::from(*character));
            }
        }
    }
    text
}

macro_rules! hex_identifier {
    ($(#[$meta:meta])* $name:ident, $bytes:expr, $field:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            /// Parses exactly the lowercase hexadecimal form.
            ///
            /// # Errors
            ///
            /// [`Refusal::FieldInvalid`] for any other text.
            pub fn parse(text: &str) -> Result<Self, Refusal> {
                if text.len() == $bytes * 2 && is_lower_hex(text) {
                    Ok(Self(text.to_owned()))
                } else {
                    Err(Refusal::FieldInvalid { field: $field })
                }
            }

            /// Builds the identifier from raw bytes (for instance random ones).
            #[must_use]
            pub fn from_bytes(bytes: [u8; $bytes]) -> Self {
                Self(hex_of(&bytes))
            }

            /// Lowercase hexadecimal form.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

hex_identifier!(
    /// Identifier of a mission: 128 random bits, 32 lowercase hexadecimal characters.
    MissionId,
    16,
    "mission"
);

hex_identifier!(
    /// Identifier of one execution of a mission: 128 random bits.
    RunId,
    16,
    "run"
);

hex_identifier!(
    /// Identifier of a deferred idea: 128 random bits.
    IdeaId,
    16,
    "idea"
);

hex_identifier!(
    /// Identifier of a decision request: 128 random bits.
    RequestId,
    16,
    "request"
);

hex_identifier!(
    /// Identifier of an agent session declared through the bridge: 128 random bits.
    SessionId,
    16,
    "session"
);

hex_identifier!(
    /// Identifier of one execution of a criterion check: 128 random bits.
    CheckId,
    16,
    "check"
);

hex_identifier!(
    /// Identifier of a phase artifact: 128 random bits.
    ArtifactId,
    16,
    "artifact"
);

/// A git commit: 40 (SHA-1) or 64 (SHA-256) lowercase hexadecimal characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitId(String);

impl CommitId {
    /// Parses a full commit identifier; abbreviations are refused.
    ///
    /// # Errors
    ///
    /// [`Refusal::FieldInvalid`] (`commit`).
    pub fn parse(text: &str) -> Result<Self, Refusal> {
        if (text.len() == 40 || text.len() == 64) && is_lower_hex(text) {
            Ok(Self(text.to_owned()))
        } else {
            Err(Refusal::FieldInvalid { field: "commit" })
        }
    }

    /// Lowercase hexadecimal form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A SHA-256 digest, as named in the journal (64 lowercase hexadecimal characters).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Digest32([u8; 32]);

impl Digest32 {
    /// Parses the lowercase hexadecimal form.
    ///
    /// # Errors
    ///
    /// [`Refusal::FieldInvalid`] (`digest`).
    pub fn parse(text: &str) -> Result<Self, Refusal> {
        let invalid = Refusal::FieldInvalid { field: "digest" };
        if text.len() != 64 || !is_lower_hex(text) {
            return Err(invalid);
        }
        let mut out = [0_u8; 32];
        for (slot, pair) in out.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
            let pair = std::str::from_utf8(pair).map_err(|_| invalid)?;
            *slot = u8::from_str_radix(pair, 16).map_err(|_| invalid)?;
        }
        Ok(Self(out))
    }

    /// Builds the digest from its raw bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Lowercase hexadecimal form.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex_of(&self.0)
    }
}
