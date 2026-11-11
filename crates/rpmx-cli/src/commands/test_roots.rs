//! Select and group installed-stage tests by the root they need.
//!
//! A test may add packages through `test.packages`, the shape Wolfi/melange
//! uses for test environments. An output-level test also selects the RPMs
//! for that output and its runtime dependencies instead of installing every
//! sibling output. Tests with the same package set and output share one
//! prepared root.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rpmx_core::error::codes;
use rpmx_core::{Error, PlannedTest};

pub(crate) struct TestGroup {
    /// Stable identity for an output plus package set, used to find the
    /// prepared root.
    pub(crate) key: String,
    /// The output this group tests. `None` is a recipe-level test, which sees
    /// every output.
    pub(crate) output: Option<String>,
    pub(crate) packages: Vec<String>,
    pub(crate) tests: Vec<PlannedTest>,
}

/// The header information needed to choose sibling outputs for an
/// output-level test.
#[derive(Debug, Clone)]
pub(crate) struct TestRpm {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    provides: BTreeSet<String>,
    requires: BTreeSet<String>,
    files: BTreeSet<String>,
}

impl TestRpm {
    fn read(path: &Path) -> Result<Self, Error> {
        let metadata = rpmx_rpm::package_metadata(path).map_err(|error| {
            Error::new(
                codes::TEST_004,
                format!(
                    "cannot inspect test RPM {}: {}",
                    path.display(),
                    error.message
                ),
            )
        })?;
        let mut provides = metadata
            .provides
            .iter()
            .map(|dependency| dependency.name.clone())
            .collect::<BTreeSet<_>>();
        provides.insert(metadata.name.clone());
        let requires = metadata.required_names();
        Ok(Self {
            name: metadata.name,
            path: path.to_path_buf(),
            provides,
            requires,
            files: metadata.files,
        })
    }
}

pub(crate) fn read_rpms(paths: &[PathBuf]) -> Result<Vec<TestRpm>, Error> {
    paths.iter().map(|path| TestRpm::read(path)).collect()
}

/// Collect the packages under test into a local repository.
///
/// The resolver installs them like published packages, so their requirements
/// resolve by name, soname and file without the recipe naming any of them. The
/// directory holds exactly the RPMs under test, which keeps a requirement on a
/// sibling output from resolving to an unpublished package.
pub(crate) fn under_test_repo(path: &Path, rpms: &[PathBuf]) -> Result<PathBuf, Error> {
    if path.exists() {
        std::fs::remove_dir_all(path).map_err(|error| {
            Error::new(
                codes::TEST_004,
                format!("cannot clear {}: {error}", path.display()),
            )
        })?;
    }
    std::fs::create_dir_all(path).map_err(|error| {
        Error::new(
            codes::TEST_004,
            format!("cannot create {}: {error}", path.display()),
        )
    })?;
    for rpm in rpms {
        let name = rpm.file_name().ok_or_else(|| {
            Error::new(
                codes::TEST_004,
                format!("{} is not a file name", rpm.display()),
            )
        })?;
        let target = path.join(name);
        if std::fs::hard_link(rpm, &target).is_err() {
            std::fs::copy(rpm, &target).map_err(|error| {
                Error::new(
                    codes::TEST_004,
                    format!("cannot copy {}: {error}", rpm.display()),
                )
            })?;
        }
    }
    crate::commands::repo::index(path, None, 0)?;
    Ok(path.to_path_buf())
}

pub(crate) fn packages_for(test: &PlannedTest) -> Vec<String> {
    let mut packages = test.packages.clone();
    packages.sort();
    packages.dedup();
    packages
}

fn owner_key(test: &PlannedTest) -> &str {
    test.output.as_deref().unwrap_or("*")
}

pub(crate) fn key_for(test: &PlannedTest) -> String {
    format!(
        "{}\u{1e}{}",
        owner_key(test),
        packages_for(test).join("\u{1f}")
    )
}

pub(crate) fn grouped(tests: &[PlannedTest]) -> Vec<TestGroup> {
    let mut groups: BTreeMap<String, TestGroup> = BTreeMap::new();
    for test in tests
        .iter()
        .filter(|test| test.stage != "build" && test.skip.is_none())
    {
        let packages = packages_for(test);
        let key = key_for(test);
        groups
            .entry(key.clone())
            .or_insert_with(|| TestGroup {
                key,
                output: test.output.clone(),
                packages,
                tests: Vec::new(),
            })
            .tests
            .push(test.clone());
    }
    groups.into_values().collect()
}

fn ambiguous(selector: &str, matches: &[&PlannedTest]) -> Error {
    let choices = matches
        .iter()
        .map(|test| test.qualified_name())
        .collect::<Vec<_>>()
        .join(", ");
    Error::new(
        codes::TEST_004,
        format!("test '{selector}' is ambiguous; use one of: {choices}"),
    )
}

/// Validate selectors across matrix cells. A bare name may repeat across
/// cells, because each cell builds independently. Within one cell it must be
/// unique unless the caller uses `<output>/<name>`.
pub(crate) fn validate_selectors(
    cells: &[&[PlannedTest]],
    selectors: &[String],
) -> Result<(), Error> {
    for selector in selectors {
        let mut found = false;
        for tests in cells {
            let matches = tests
                .iter()
                .filter(|test| test.matches_selector(selector))
                .collect::<Vec<_>>();
            if matches.is_empty() {
                continue;
            }
            found = true;
            if matches.len() > 1 && !selector.contains('/') {
                return Err(ambiguous(selector, &matches));
            }
        }
        if !found {
            return Err(Error::new(
                codes::TEST_004,
                format!("no test named '{selector}' in this recipe"),
            ));
        }
    }
    Ok(())
}

/// Keep the tests selected by `selectors`. A selector that belongs to another
/// matrix cell yields an empty list rather than an error.
pub(crate) fn filter_tests(
    tests: &[PlannedTest],
    selectors: &[String],
) -> Result<Vec<PlannedTest>, Error> {
    if selectors.is_empty() {
        return Ok(tests.to_vec());
    }
    for selector in selectors {
        let matches = tests
            .iter()
            .filter(|test| test.matches_selector(selector))
            .collect::<Vec<_>>();
        if matches.len() > 1 && !selector.contains('/') {
            return Err(ambiguous(selector, &matches));
        }
    }
    Ok(tests
        .iter()
        .filter(|test| {
            selectors
                .iter()
                .any(|selector| test.matches_selector(selector))
        })
        .cloned()
        .collect())
}

/// RPMs for one test root. A recipe-level test sees every output; an
/// output-level test sees its own output plus the sibling outputs that provide
/// something it requires.
pub(crate) fn rpms_for_output(
    output: Option<&str>,
    rpms: &[TestRpm],
    test_packages: &[String],
) -> Result<Vec<PathBuf>, Error> {
    let Some(output) = output else {
        return Ok(rpms.iter().map(|rpm| rpm.path.clone()).collect());
    };
    let start = rpms
        .iter()
        .position(|rpm| rpm.name == output)
        .ok_or_else(|| {
            Error::new(
                codes::TEST_004,
                format!("output '{output}' produced no binary RPM for its test root"),
            )
        })?;
    let mut providers: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, rpm) in rpms.iter().enumerate() {
        for capability in rpm.provides.iter().chain(rpm.files.iter()) {
            providers.entry(capability.as_str()).or_insert(index);
        }
    }
    let mut selected = BTreeSet::from([start]);
    // An explicitly requested sibling is part of the test environment.
    // Select it by package name, without replacing external virtual providers.
    for request in test_packages {
        if let Some(index) = rpms.iter().position(|rpm| rpm.name == *request) {
            selected.insert(index);
        }
    }
    loop {
        let requires = selected
            .iter()
            .flat_map(|index| rpms[*index].requires.iter().cloned())
            .collect::<Vec<_>>();
        let mut changed = false;
        for requirement in requires {
            if let Some(provider) = providers.get(requirement.as_str()) {
                changed |= selected.insert(*provider);
            }
        }
        if !changed {
            break;
        }
    }
    Ok(selected
        .iter()
        .map(|index| rpms[*index].path.clone())
        .collect())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        filter_tests, grouped, key_for, packages_for, rpms_for_output, validate_selectors, TestRpm,
    };
    use rpmx_core::PlannedTest;

    fn test(name: &str, packages: &[&str]) -> PlannedTest {
        test_for(None, name, packages)
    }

    fn test_for(output: Option<&str>, name: &str, packages: &[&str]) -> PlannedTest {
        PlannedTest {
            name: name.to_string(),
            files: None,
            artifacts: Vec::new(),
            skip: None,
            output: output.map(str::to_string),
            stage: "installed".to_string(),
            argv: None,
            pipeline: Vec::new(),
            packages: packages.iter().map(|value| value.to_string()).collect(),
            environment: Default::default(),
        }
    }

    fn rpm(name: &str, provides: &[&str], requires: &[&str], files: &[&str]) -> TestRpm {
        TestRpm {
            name: name.to_string(),
            path: PathBuf::from(format!("{name}.rpm")),
            provides: provides.iter().map(|value| value.to_string()).collect(),
            requires: requires.iter().map(|value| value.to_string()).collect(),
            files: files.iter().map(|value| value.to_string()).collect(),
        }
    }

    #[test]
    fn tests_with_the_same_package_set_and_output_share_a_root() {
        let groups = grouped(&[test("one", &["python3"]), test("two", &["python3"])]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].packages, ["python3"]);
        assert_eq!(groups[0].tests.len(), 2);
        assert_eq!(groups[0].key, key_for(&test("one", &["python3"])));
    }

    #[test]
    fn a_different_package_set_or_output_gets_its_own_root() {
        let groups = grouped(&[
            test("plain", &[]),
            test("python", &["python3"]),
            test_for(Some("lib"), "symbols", &[]),
        ]);
        assert_eq!(groups.len(), 3);
        assert!(packages_for(&test("plain", &[])).is_empty());
        assert_eq!(packages_for(&test("python", &["python3"])), ["python3"]);
    }

    #[test]
    fn a_bare_name_is_rejected_when_output_tests_share_it() {
        let tests = [
            test_for(Some("one"), "smoke", &[]),
            test_for(Some("two"), "smoke", &[]),
        ];
        let cells: &[&[PlannedTest]] = &[&tests];
        let error = validate_selectors(cells, &["smoke".to_string()]).expect_err("ambiguous");
        assert!(error.message.contains("one/smoke, two/smoke"));
        validate_selectors(cells, &["one/smoke".to_string()]).expect("qualified");
        assert_eq!(
            filter_tests(&tests, &["one/smoke".to_string()])
                .expect("filter")
                .len(),
            1
        );
    }

    #[test]
    fn the_same_name_may_repeat_across_matrix_cells() {
        let one = [test("smoke", &[])];
        let two = [test("smoke", &[])];
        let cells: &[&[PlannedTest]] = &[&one, &two];
        validate_selectors(cells, &["smoke".to_string()]).expect("matrix cells");
        assert_eq!(
            filter_tests(&one, &["smoke".to_string()])
                .expect("filter")
                .len(),
            1
        );
        assert_eq!(
            filter_tests(&two, &["smoke".to_string()])
                .expect("filter")
                .len(),
            1
        );
    }

    #[test]
    fn an_output_test_gets_its_runtime_siblings_but_not_unrelated_outputs() {
        let rpms = [
            rpm("app", &["app"], &["libapp.so.1"], &["/usr/bin/app"]),
            rpm("libs", &["libapp.so.1"], &[], &["/usr/lib64/libapp.so.1"]),
            rpm("docs", &["docs"], &[], &["/usr/share/doc/docs"]),
        ];
        let selected = rpms_for_output(Some("app"), &rpms, &[]).expect("selection");
        assert_eq!(
            selected,
            [PathBuf::from("app.rpm"), PathBuf::from("libs.rpm")]
        );
    }

    #[test]
    fn an_output_test_without_siblings_does_not_install_them() {
        let rpms = [
            rpm("app", &["app"], &[], &["/usr/bin/app"]),
            rpm("docs", &["docs"], &[], &["/usr/share/doc/docs"]),
        ];
        let selected = rpms_for_output(Some("app"), &rpms, &[]).expect("selection");
        assert_eq!(selected, [PathBuf::from("app.rpm")]);
    }

    #[test]
    fn a_file_requirement_can_pull_a_sibling_output() {
        let rpms = [
            rpm("app", &["app"], &["/usr/bin/helper"], &["/usr/bin/app"]),
            rpm("helper", &["helper"], &[], &["/usr/bin/helper"]),
        ];
        let selected = rpms_for_output(Some("app"), &rpms, &[]).expect("selection");
        assert_eq!(
            selected,
            [PathBuf::from("app.rpm"), PathBuf::from("helper.rpm")]
        );
    }

    #[test]
    fn a_recipe_level_test_still_sees_every_output() {
        let rpms = [
            rpm("app", &["app"], &[], &[]),
            rpm("docs", &["docs"], &[], &[]),
        ];
        let selected = rpms_for_output(None, &rpms, &[]).expect("selection");
        assert_eq!(
            selected,
            [PathBuf::from("app.rpm"), PathBuf::from("docs.rpm")]
        );
    }

    #[test]
    fn a_test_can_request_a_sibling_and_its_runtime_dependencies() {
        let rpms = [
            rpm("bash", &["bash"], &[], &[]),
            rpm("bash-binsh", &["bash-binsh"], &["bash"], &[]),
            rpm("docs", &["docs"], &[], &[]),
        ];
        let selected = rpms_for_output(
            Some("bash"),
            &rpms,
            &["bash-binsh".to_string(), "grep".to_string()],
        )
        .expect("selection");
        assert_eq!(
            selected,
            [PathBuf::from("bash.rpm"), PathBuf::from("bash-binsh.rpm")]
        );
    }
}
