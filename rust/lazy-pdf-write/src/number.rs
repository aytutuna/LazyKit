//! Numbers in PDF syntax, written the way Lazy Scan's writers do: at most four decimals, none when they are zero.

/// "0.####" as .NET writes it: the value taken to 15 significant digits first, then rounded half away from zero
/// at the fourth decimal, trailing zeros dropped. A value that rounds to zero keeps its sign ("-0").
pub fn num(value: f64) -> String {
    if !value.is_finite() {
        return "0".into();
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    let scientific = format!("{:.14e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap();
    let exponent: i32 = exponent.parse().unwrap();
    let mut digits: Vec<u8> = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| b - b'0')
        .collect();
    // The value is 0.d1d2d3... times 10^(exponent + 1); keep the digits that lie at or above 1e-4.
    let point = exponent + 1;
    let keep = point + 4;
    let mut carry = false;
    if keep < 0 {
        digits.clear();
    } else if (keep as usize) < digits.len() {
        carry = digits[keep as usize] >= 5;
        digits.truncate(keep as usize);
    }
    let mut point = point;
    if carry {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, 1);
                point += 1;
                break;
            }
            i -= 1;
            if digits[i] == 9 {
                digits[i] = 0;
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let digit = |index: i32| {
        usize::try_from(index)
            .ok()
            .and_then(|i| digits.get(i).copied())
            .unwrap_or(0)
    };
    let mut integer: String = (0..point.max(0))
        .map(|i| (b'0' + digit(i)) as char)
        .collect();
    let fraction: String = (0..4).map(|k| (b'0' + digit(point + k)) as char).collect();
    let fraction = fraction.trim_end_matches('0');
    if integer.is_empty() {
        integer.push('0');
    }
    if fraction.is_empty() {
        format!("{sign}{integer}")
    } else {
        format!("{sign}{integer}.{fraction}")
    }
}
