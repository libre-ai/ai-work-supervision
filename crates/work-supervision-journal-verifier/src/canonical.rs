//! RFC 8785 (JCS) encoding restricted to the journal's value domain.
//!
//! Written from the RFC, independently of `serde_jcs`, which the writer uses:
//! a canonicalisation defect in either implementation becomes a disagreement
//! that the cross-implementation tests catch. Floats are outside the domain,
//! so numbers are integers written in decimal.

use std::cmp::Ordering;

use serde_json::Value;

/// Canonical bytes of `value`, or `None` when it holds a float.
#[must_use]
pub fn encode(value: &Value) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    write_value(value, &mut out)?;
    Some(out)
}

fn write_value(value: &Value, out: &mut Vec<u8>) -> Option<()> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(number) => {
            let decimal = match (number.as_i64(), number.as_u64()) {
                (Some(integer), _) => integer.to_string(),
                (None, Some(integer)) => integer.to_string(),
                (None, None) => return None,
            };
            out.extend_from_slice(decimal.as_bytes());
        }
        Value::String(text) => write_string(text, out),
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_value(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(fields) => {
            // RFC 8785 §3.2.3: members sorted by their names as UTF-16 code units.
            let mut members: Vec<(&String, &Value)> = fields.iter().collect();
            members.sort_by(|left, right| utf16_order(left.0, right.0));
            out.push(b'{');
            for (index, (name, member)) in members.into_iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_string(name, out);
                out.push(b':');
                write_value(member, out)?;
            }
            out.push(b'}');
        }
    }
    Some(())
}

fn utf16_order(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

/// RFC 8785 §3.2.2.2 (the ECMAScript `JSON.stringify` string rules).
fn write_string(text: &str, out: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'"');
    let mut buffer = [0_u8; 4];
    for character in text.chars() {
        match character {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            control if u32::from(control) < 0x20 => {
                let code = u32::from(control);
                out.extend_from_slice(b"\\u00");
                for shift in [4_u32, 0] {
                    let nibble = usize::try_from((code >> shift) & 0xf).unwrap_or(0);
                    out.push(HEX.get(nibble).copied().unwrap_or(b'0'));
                }
            }
            other => out.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes()),
        }
    }
    out.push(b'"');
}
