//! Reads a code that a person typed, or that was found in a log, as 32-bit values.

/// Characters often wrapped around a code, as in "(0x80070005)" or "5.", which
/// are trimmed before reading it.
const WRAPPING: &[char] = &[
    '(', ')', '[', ']', '{', '}', '<', '>', '"', '\'', ',', ';', ':', '.',
];

/// Every value the text could mean, most likely first. Empty if it isn't a code.
///
/// "0x" means hex, and a leading minus means a signed decimal. Bare digits are
/// ambiguous, so both readings are returned: hex first when there are exactly
/// eight digits, as in "80070005", and decimal first otherwise, as in "5" or
/// "2147942405". Digits with any of a to f are hex.
pub(crate) fn parse_values(text: &str) -> Vec<u32> {
    let token = text.trim_matches(|c: char| c.is_whitespace() || WRAPPING.contains(&c));
    let mut values = Vec::new();
    if let Some(magnitude) = token.strip_prefix('-') {
        values.extend(parse_negative(magnitude));
        return values;
    }
    let token = token.strip_prefix('+').unwrap_or(token);
    if let Some(hex) = token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("0X"))
    {
        values.extend(parse_hex(hex));
        return values;
    }
    if is_decimal(token) {
        let decimal = parse_decimal(token);
        let hex = parse_hex(token);
        if token.len() == 8 {
            values.extend(hex);
            values.extend(decimal);
        } else {
            values.extend(decimal);
            values.extend(hex);
        }
        values.dedup();
    } else {
        values.extend(parse_hex(token));
    }
    values
}

fn is_decimal(digits: &str) -> bool {
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

/// Reads hex digits, allowing leading zeros, as long as the value fits 32 bits.
fn parse_hex(digits: &str) -> Option<u32> {
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let significant = digits.trim_start_matches('0');
    if significant.is_empty() {
        return Some(0);
    }
    if significant.len() > 8 {
        return None;
    }
    u32::from_str_radix(significant, 16).ok()
}

/// Reads decimal digits, allowing leading zeros, as long as the value fits 32 bits.
fn parse_decimal(digits: &str) -> Option<u32> {
    if !is_decimal(digits) {
        return None;
    }
    let significant = digits.trim_start_matches('0');
    if significant.is_empty() {
        return Some(0);
    }
    if significant.len() > 10 {
        return None;
    }
    significant.parse::<u32>().ok()
}

/// Reads the digits after a minus sign as a signed 32-bit value, and returns
/// its bits as an unsigned code.
fn parse_negative(digits: &str) -> Option<u32> {
    if !is_decimal(digits) {
        return None;
    }
    let significant = digits.trim_start_matches('0');
    if significant.len() > 10 {
        return None;
    }
    let magnitude: i64 = if significant.is_empty() {
        0
    } else {
        significant.parse().ok()?
    };
    let value = i32::try_from(-magnitude).ok()?;
    // The same bits, read as unsigned.
    Some(value as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_with_a_prefix() {
        assert_eq!(parse_values("0x80070005"), vec![0x8007_0005]);
        assert_eq!(parse_values("0XC0000005"), vec![0xC000_0005]);
        assert_eq!(parse_values("0x5"), vec![5]);
        assert_eq!(parse_values("0x0000000080070005"), vec![0x8007_0005]);
    }

    #[test]
    fn eight_bare_digits_read_as_hex_first() {
        assert_eq!(parse_values("80070005"), vec![0x8007_0005, 80_070_005]);
        assert_eq!(parse_values("c0000005"), vec![0xC000_0005]);
    }

    #[test]
    fn decimals() {
        assert_eq!(parse_values("5"), vec![5]);
        assert_eq!(parse_values("12002"), vec![12002, 0x12002]);
        assert_eq!(parse_values("2147942405"), vec![0x8007_0005]);
        // The signed form's digits without the minus are a different value.
        assert_eq!(parse_values("2147024891"), vec![0x7FF8_FFFB]);
        assert_eq!(parse_values("-2147024891"), vec![0x8007_0005]);
        assert_eq!(parse_values("-2147483648"), vec![0x8000_0000]);
        assert_eq!(parse_values("4294967295"), vec![u32::MAX]);
    }

    #[test]
    fn wrapping_characters_are_trimmed() {
        assert_eq!(parse_values(" (0x80070005). "), vec![0x8007_0005]);
        assert_eq!(parse_values("\"5\""), vec![5]);
    }

    #[test]
    fn non_codes_read_as_nothing() {
        for text in [
            "",
            "-",
            "0x",
            "hello",
            "0x123456789",
            "4294967296",
            "-2147483649",
            "99999999999",
            "--5",
            "0x-5",
            "1 2",
            "５",
        ] {
            assert!(parse_values(text).is_empty(), "{text:?}");
        }
    }
}
