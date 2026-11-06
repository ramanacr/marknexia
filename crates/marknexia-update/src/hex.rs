//! Lower-case hexadecimal, the only encoding used in update documents.

/// Encodes `bytes` as lower-case hex.
#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

/// Decodes exactly `N` bytes of lower-case hex. Upper case, odd lengths and
/// any other character are refused, so each value has one spelling.
#[must_use]
pub fn decode<const N: usize>(text: &str) -> Option<[u8; N]> {
    let digits = text.as_bytes();
    if digits.len() != N * 2 {
        return None;
    }
    let mut bytes = [0_u8; N];
    for (slot, pair) in bytes.iter_mut().zip(digits.chunks_exact(2)) {
        *slot = (digit(pair[0])? << 4) | digit(pair[1])?;
    }
    Some(bytes)
}

fn digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Compares two byte strings in time that depends only on their lengths.
#[must_use]
pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_in_lower_case_only() {
        let bytes = [0x00, 0x7f, 0x80, 0xff, 0xab];
        assert_eq!(encode(&bytes), "007f80ffab");
        assert_eq!(decode::<5>("007f80ffab"), Some(bytes));
        assert_eq!(decode::<5>("007F80FFAB"), None);
        assert_eq!(decode::<5>("007f80ffa"), None);
        assert_eq!(decode::<5>("007f80ffabcd"), None);
        assert_eq!(decode::<1>("g0"), None);
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
