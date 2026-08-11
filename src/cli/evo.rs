//! Key handling for Evolution frame protection verification.
//!
//! The check itself lives in [`truehd::structs::extra_data::ExtraData::verify_evo_protection`];
//! this supplies the key and tallies the results.

use truehd::structs::access_unit::AccessUnit;
use truehd::structs::evolution::{EvoProtection, EvoProtectionStatus};

/// An HMAC key, kept in a newtype so clap treats it as one value rather than a byte list.
#[derive(Debug, Clone)]
pub struct EvoKey(pub Vec<u8>);

/// Reads a key from a hex string, or from a file holding one when prefixed with `@`.
pub fn parse_key(value: &str) -> Result<EvoKey, String> {
    let text = match value.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read key file {path}: {e}"))?,
        None => value.to_string(),
    };

    let hex: String = text.chars().filter(|c| !c.is_whitespace()).collect();

    if hex.is_empty() {
        return Err("key is empty".to_string());
    }

    if !hex.len().is_multiple_of(2) {
        return Err("key must have an even number of hex digits".to_string());
    }

    let key = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|_| "key must be hexadecimal".to_string())?;

    if key.len() > 64 {
        return Err("key must be at most 64 bytes".to_string());
    }

    Ok(EvoKey(key))
}

pub struct EvoVerifier {
    key: Vec<u8>,
    checked: usize,
    failed: usize,
    secondary_seen: bool,
}

impl EvoVerifier {
    pub fn new(key: EvoKey) -> Self {
        Self {
            key: key.0,
            checked: 0,
            failed: 0,
            secondary_seen: false,
        }
    }

    pub fn check(&mut self, access_unit: &AccessUnit, frame: &[u8]) -> EvoProtectionStatus {
        let Some(extra_data) = &access_unit.extra_data else {
            return EvoProtectionStatus::Absent;
        };

        if let Some(evo_frame) = &extra_data.evo_frame
            && EvoProtection::SIZE[evo_frame.evo_protection.protection_length_secondary as usize]
                != 0
        {
            self.secondary_seen = true;
        }

        let status = extra_data.verify_evo_protection(frame, &self.key);

        match status {
            EvoProtectionStatus::Absent => {}
            EvoProtectionStatus::Match => self.checked += 1,
            EvoProtectionStatus::Mismatch { .. } => {
                self.checked += 1;
                self.failed += 1;
            }
        }

        status
    }

    pub fn checked(&self) -> usize {
        self.checked
    }

    pub fn failed(&self) -> usize {
        self.failed
    }

    /// Whether any frame carried a secondary word, which is parsed but not verified.
    pub fn secondary_seen(&self) -> bool {
        self.secondary_seen
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_keys() {
        assert_eq!(parse_key("00ff10").unwrap().0, vec![0x00, 0xff, 0x10]);
        assert_eq!(parse_key(" 00 ff\n10 ").unwrap().0, vec![0x00, 0xff, 0x10]);
    }

    #[test]
    fn rejects_malformed_keys() {
        assert!(parse_key("").is_err());
        assert!(parse_key("abc").is_err());
        assert!(parse_key("zz").is_err());
        assert!(parse_key(&"00".repeat(65)).is_err());
    }
}
