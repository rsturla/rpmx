//! Group installed-stage tests by the root they need.
//!
//! A test may add packages through `test.environment.contents.packages`, the
//! same shape Wolfi/melange uses. Tests with the same package set share one
//! prepared root; a different set gets a separate disposable root instead of
//! silently inheriting another test's dependencies.

use std::collections::BTreeMap;

use rpmx_core::PlannedTest;

pub(crate) struct TestGroup {
    /// Stable identity for a package set, used to find the prepared root.
    pub(crate) key: String,
    pub(crate) packages: Vec<String>,
    pub(crate) tests: Vec<PlannedTest>,
}

pub(crate) fn packages_for(global: &[String], test: &PlannedTest) -> Vec<String> {
    let mut packages = global.to_vec();
    packages.extend(test.packages.iter().cloned());
    packages.sort();
    packages.dedup();
    packages
}

pub(crate) fn key_for(global: &[String], test: &PlannedTest) -> String {
    packages_for(global, test).join("\u{1f}")
}

pub(crate) fn grouped(tests: &[PlannedTest], global: &[String]) -> Vec<TestGroup> {
    let mut groups: BTreeMap<String, TestGroup> = BTreeMap::new();
    for test in tests.iter().filter(|test| test.stage == "installed") {
        let packages = packages_for(global, test);
        let key = key_for(global, test);
        groups
            .entry(key.clone())
            .or_insert_with(|| TestGroup {
                key,
                packages,
                tests: Vec::new(),
            })
            .tests
            .push(test.clone());
    }
    groups.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::{grouped, key_for, packages_for};
    use rpmx_core::PlannedTest;

    fn test(name: &str, packages: &[&str]) -> PlannedTest {
        PlannedTest {
            name: name.to_string(),
            stage: "installed".to_string(),
            argv: None,
            pipeline: Vec::new(),
            packages: packages.iter().map(|value| value.to_string()).collect(),
            environment: Default::default(),
        }
    }

    #[test]
    fn tests_with_the_same_package_set_share_a_root() {
        let global = vec!["bash".to_string()];
        let groups = grouped(
            &[test("one", &["python3"]), test("two", &["python3"])],
            &global,
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].packages, ["bash", "python3"]);
        assert_eq!(groups[0].tests.len(), 2);
        assert_eq!(groups[0].key, key_for(&global, &test("one", &["python3"])));
    }

    #[test]
    fn a_different_package_set_gets_its_own_root() {
        let global = vec!["bash".to_string()];
        let groups = grouped(&[test("plain", &[]), test("python", &["python3"])], &global);
        assert_eq!(groups.len(), 2);
        assert_eq!(packages_for(&global, &test("plain", &[])), ["bash"]);
        assert_eq!(
            packages_for(&global, &test("python", &["python3"])),
            ["bash", "python3"]
        );
    }
}
