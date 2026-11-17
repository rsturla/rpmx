//! Shared selection policy for source checks and installed-package tests.
use rpmx_language::PlannedTest;

pub(crate) fn includes(test: &PlannedTest, selectors: &[String], include_optional: bool) -> bool {
    if selectors.is_empty() {
        !test.optional || include_optional
    } else {
        selectors
            .iter()
            .any(|selector| test.matches_selector(selector))
    }
}
