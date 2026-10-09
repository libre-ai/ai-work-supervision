use sha2::{Digest as _, Sha256};

/// A SHA-256 digest, written in the journal as 64 lowercase hexadecimal characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Digest([u8; 32]);

const HEX: &[u8; 16] = b"0123456789abcdef";

impl Digest {
    /// SHA-256 of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// Lowercase hexadecimal form, as written in the journal.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut text = String::with_capacity(64);
        for byte in self.0 {
            for nibble in [byte >> 4, byte & 0x0f] {
                if let Some(character) = HEX.get(usize::from(nibble)) {
                    text.push(char::from(*character));
                }
            }
        }
        text
    }

    /// Parses exactly 64 lowercase hexadecimal characters; anything else is `None`.
    #[must_use]
    pub fn from_hex(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != 64 {
            return None;
        }
        let mut out = [0_u8; 32];
        for (slot, pair) in out.iter_mut().zip(bytes.chunks_exact(2)) {
            let [high, low] = pair else {
                return None;
            };
            *slot = (nibble(*high)? << 4) | nibble(*low)?;
        }
        Some(Self(out))
    }

    /// Raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

const fn nibble(character: u8) -> Option<u8> {
    match character {
        b'0'..=b'9' => Some(character - b'0'),
        b'a'..=b'f' => Some(character - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::Digest;

    #[test]
    fn hex_round_trips_and_refuses_uppercase_and_wrong_lengths() {
        let digest = Digest::of(b"abc");
        let hex = digest.to_hex();
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(Digest::from_hex(&hex), Some(digest));
        assert_eq!(Digest::from_hex(&hex.to_uppercase()), None);
        assert_eq!(Digest::from_hex(&hex[..62]), None);
        assert_eq!(Digest::from_hex(&format!("{hex}00")), None);
        assert_eq!(Digest::from_hex(&hex.replacen('b', "g", 1)), None);
    }
}
