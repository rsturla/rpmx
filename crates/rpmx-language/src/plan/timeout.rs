use std::num::NonZeroU64;

use rpmx_base::error::{codes, Error, Result};

/// Recipe deadlines use whole seconds, minutes or hours, without implicit units.
pub(super) fn parse(value: &str) -> Result<NonZeroU64> {
    let invalid = || {
        Error::new(
        codes::STEP_020,
        format!("invalid step timeout '{value}': use a positive whole-number duration such as 1s, 30m or 4h"),
    )
    };
    let (digits, multiplier) = match value.as_bytes().last() {
        Some(b's') => (&value[..value.len() - 1], 1),
        Some(b'm') => (&value[..value.len() - 1], 60),
        Some(b'h') => (&value[..value.len() - 1], 3600),
        _ => return Err(invalid()),
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    digits
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(multiplier))
        .and_then(NonZeroU64::new)
        .ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::model::Step;
    use crate::plan::plan_steps;

    #[test]
    fn duration_units_and_overflow_are_checked() {
        for (value, seconds) in [("1s", 1), ("30m", 1800), ("4h", 14400)] {
            assert_eq!(parse(value).unwrap().get(), seconds);
        }
        for value in [
            "",
            "0s",
            "0m",
            "0h",
            "1",
            "-1s",
            "+1s",
            "1.5s",
            "1ms",
            "1d",
            " 1s",
            "1s ",
            "s",
            "18446744073709551616s",
            "18446744073709551615h",
        ] {
            assert_eq!(parse(value).unwrap_err().code, "RPMX-STEP-020", "{value}");
        }
    }

    #[test]
    fn every_step_form_carries_its_deadline_and_default_is_distinct() {
        let steps: Vec<Step> = serde_json::from_value(serde_json::json!([
            {"runs": "true", "timeout": "4h"},
            {"argv": ["true"], "timeout": "30m"},
            {"uses": "test/runtime-linkage", "with": {"libraries": ["libz.so.*"]}, "timeout": "1s"},
            {"runs": "true"}
        ]))
        .unwrap();
        let planned = plan_steps(&steps, None, "tests[0].pipeline", false, "test").unwrap();
        assert_eq!(
            planned
                .iter()
                .map(|step| step.timeout_seconds.map(|n| n.get()))
                .collect::<Vec<_>>(),
            vec![Some(14400), Some(1800), Some(1), None]
        );
    }

    #[test]
    fn invalid_deadline_identifies_the_recipe_field() {
        let steps: Vec<Step> = serde_json::from_value(serde_json::json!([
            {"runs": "true", "timeout": "0s"}
        ]))
        .unwrap();
        let error = plan_steps(&steps, None, "tests[0].pipeline", false, "test").unwrap_err();
        assert_eq!(error.code, "RPMX-STEP-020");
        assert!(format!("{error:?}").contains("tests[0].pipeline[0].timeout"));
    }
}
