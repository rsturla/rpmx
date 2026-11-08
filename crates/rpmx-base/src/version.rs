//! RPM version comparison.
//!
//! This follows rpm's `rpmvercmp` rules, including the `~` and `^`
//! separators. It is used to decide whether a built package's `Requires` is
//! already satisfied by another selected package; the repositories must only
//! be asked for requirements that are genuinely external.

use std::cmp::Ordering;

/// Compare two RPM version strings using rpm's segment rules.
pub fn compare(left: &str, right: &str) -> Ordering {
    if left == right {
        return Ordering::Equal;
    }

    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut left_pos = 0;
    let mut right_pos = 0;

    loop {
        while left_pos < left.len()
            && !is_alphanumeric(left[left_pos])
            && left[left_pos] != b'~'
            && left[left_pos] != b'^'
        {
            left_pos += 1;
        }
        while right_pos < right.len()
            && !is_alphanumeric(right[right_pos])
            && right[right_pos] != b'~'
            && right[right_pos] != b'^'
        {
            right_pos += 1;
        }

        if left.get(left_pos) == Some(&b'~') || right.get(right_pos) == Some(&b'~') {
            return match (left.get(left_pos), right.get(right_pos)) {
                (Some(b'~'), Some(b'~')) => {
                    left_pos += 1;
                    right_pos += 1;
                    continue;
                }
                (Some(b'~'), _) => Ordering::Less,
                (_, Some(b'~')) => Ordering::Greater,
                _ => unreachable!(),
            };
        }

        if left.get(left_pos) == Some(&b'^') || right.get(right_pos) == Some(&b'^') {
            return match (left.get(left_pos), right.get(right_pos)) {
                (Some(b'^'), Some(b'^')) => {
                    left_pos += 1;
                    right_pos += 1;
                    continue;
                }
                (Some(b'^'), None) => Ordering::Greater,
                (Some(b'^'), Some(_)) => Ordering::Less,
                (None, Some(b'^')) => Ordering::Less,
                (Some(_), Some(b'^')) => Ordering::Greater,
                _ => unreachable!(),
            };
        }

        if left_pos >= left.len() || right_pos >= right.len() {
            break;
        }

        let left_start = left_pos;
        let right_start = right_pos;
        let numeric = is_digit(left[left_start]);
        if numeric {
            while left_pos < left.len() && is_digit(left[left_pos]) {
                left_pos += 1;
            }
            while right_pos < right.len() && is_digit(right[right_pos]) {
                right_pos += 1;
            }
        } else {
            while left_pos < left.len() && is_alpha(left[left_pos]) {
                left_pos += 1;
            }
            while right_pos < right.len() && is_alpha(right[right_pos]) {
                right_pos += 1;
            }
        }

        let left_segment = &left[left_start..left_pos];
        let right_segment = &right[right_start..right_pos];

        if left_segment.is_empty() {
            return Ordering::Less;
        }
        if right_segment.is_empty() {
            return if numeric {
                Ordering::Greater
            } else {
                Ordering::Less
            };
        }

        let ordering = if numeric {
            let left_digits = trim_zeroes(left_segment);
            let right_digits = trim_zeroes(right_segment);
            left_digits
                .len()
                .cmp(&right_digits.len())
                .then_with(|| left_digits.cmp(right_digits))
        } else {
            left_segment.cmp(right_segment)
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }

    match (left_pos >= left.len(), right_pos >= right.len()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => Ordering::Equal,
    }
}

/// Compare two RPM epoch-version-release strings.
pub fn compare_evr(left: &str, right: &str) -> Ordering {
    let (left_epoch, left_rest) = split_epoch(left);
    let (right_epoch, right_rest) = split_epoch(right);
    compare_numeric(left_epoch.as_bytes(), right_epoch.as_bytes()).then_with(|| {
        let (left_version, left_release) = split_release(left_rest);
        let (right_version, right_release) = split_release(right_rest);
        compare(left_version, right_version).then_with(|| compare(left_release, right_release))
    })
}

fn split_epoch(value: &str) -> (&str, &str) {
    match value.split_once(':') {
        Some((epoch, rest)) if !epoch.is_empty() && epoch.bytes().all(is_digit) => (epoch, rest),
        _ => ("0", value),
    }
}

fn split_release(value: &str) -> (&str, &str) {
    value.rsplit_once('-').unwrap_or((value, ""))
}

fn compare_numeric(left: &[u8], right: &[u8]) -> Ordering {
    let left = trim_zeroes(left);
    let right = trim_zeroes(right);
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

fn trim_zeroes(value: &[u8]) -> &[u8] {
    let start = value.iter().position(|byte| *byte != b'0');
    match start {
        Some(start) => &value[start..],
        None => &value[value.len()..],
    }
}

fn is_alpha(byte: u8) -> bool {
    byte.is_ascii_alphabetic()
}

fn is_digit(byte: u8) -> bool {
    byte.is_ascii_digit()
}

fn is_alphanumeric(byte: u8) -> bool {
    is_alpha(byte) || is_digit(byte)
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::{compare, compare_evr};

    #[test]
    fn numeric_and_alpha_segments_follow_rpm_rules() {
        assert_eq!(compare("1.0", "1.0"), Ordering::Equal);
        assert_eq!(compare("1.0", "1.1"), Ordering::Less);
        assert_eq!(compare("1.10", "1.9"), Ordering::Greater);
        assert_eq!(compare("1.0a", "1.0"), Ordering::Greater);
        assert_eq!(compare("1.0a", "1.0b"), Ordering::Less);
        assert_eq!(compare("1.0", "1.0.0"), Ordering::Less);
        assert_eq!(compare("1.0~rc1", "1.0"), Ordering::Less);
        assert_eq!(compare("1.0^git1", "1.0"), Ordering::Greater);
    }

    #[test]
    fn epoch_and_release_are_compared_separately() {
        assert_eq!(compare_evr("1:1.0-1", "1.0-99"), Ordering::Greater);
        assert_eq!(compare_evr("10:1.0-1", "9:2.0-1"), Ordering::Greater);
        assert_eq!(compare_evr("1.0-2", "1.0-1"), Ordering::Greater);
        assert_eq!(compare_evr("0:1.0-1", "1.0-1"), Ordering::Equal);
    }
}
