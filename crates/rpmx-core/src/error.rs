use std::fmt;

use serde::Serialize;

/// A stable diagnostic code such as `RPMX-STEP-011`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Code(&'static str);

impl Code {
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Serialize for Code {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.0)
    }
}

impl PartialEq<&str> for Code {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

/// Every diagnostic code in the tool.
///
/// Names are derived from the code (`RPMX-STEP-011` is `STEP_011`) so there is
/// one obvious constant per code and no second naming convention to maintain.
pub mod codes {
    use super::Code;

    macro_rules! define_codes {
        ($($name:ident = $code:literal),* $(,)?) => {
            $(pub const $name: Code = Code($code);)*
            pub const ALL: &[Code] = &[$($name),*];
        };
    }

    define_codes! {
        ASSEMBLY_011 = "RPMX-ASSEMBLY-011",
        BUILD_004 = "RPMX-BUILD-004",
        BUILD_005 = "RPMX-BUILD-005",
        BUILD_006 = "RPMX-BUILD-006",
        BUILD_007 = "RPMX-BUILD-007",
        CLI_005 = "RPMX-CLI-005",
        CLI_006 = "RPMX-CLI-006",
        EXTRACT_001 = "RPMX-EXTRACT-001",
        EXTRACT_002 = "RPMX-EXTRACT-002",
        EXTRACT_003 = "RPMX-EXTRACT-003",
        EXTRACT_004 = "RPMX-EXTRACT-004",
        EXTRACT_005 = "RPMX-EXTRACT-005",
        EXTRACT_006 = "RPMX-EXTRACT-006",
        FETCH_001 = "RPMX-FETCH-001",
        SOURCE_030 = "RPMX-SOURCE-030",
        SOURCE_031 = "RPMX-SOURCE-031",
        SOURCE_032 = "RPMX-SOURCE-032",
        FETCH_002 = "RPMX-FETCH-002",
        FETCH_003 = "RPMX-FETCH-003",
        FETCH_004 = "RPMX-FETCH-004",
        FETCH_005 = "RPMX-FETCH-005",
        FETCH_006 = "RPMX-FETCH-006",
        FETCH_007 = "RPMX-FETCH-007",
        REPO_001 = "RPMX-REPO-001",
        REPO_002 = "RPMX-REPO-002",
        REPO_003 = "RPMX-REPO-003",
        REPO_004 = "RPMX-REPO-004",
        ROOT_001 = "RPMX-ROOT-001",
        ROOT_002 = "RPMX-ROOT-002",
        ROOT_003 = "RPMX-ROOT-003",
        ROOT_004 = "RPMX-ROOT-004",
        ROOT_005 = "RPMX-ROOT-005",
        ROOT_006 = "RPMX-ROOT-006",
        ROOT_007 = "RPMX-ROOT-007",
        ROOT_008 = "RPMX-ROOT-008",
        ROOT_009 = "RPMX-ROOT-009",
        ROOT_010 = "RPMX-ROOT-010",
        ROOT_011 = "RPMX-ROOT-011",
        ROOT_012 = "RPMX-ROOT-012",
        ROOT_013 = "RPMX-ROOT-013",
        ROOT_014 = "RPMX-ROOT-014",
        ROOT_015 = "RPMX-ROOT-015",
        ROOT_016 = "RPMX-ROOT-016",
        ROOT_017 = "RPMX-ROOT-017",
        ROOT_018 = "RPMX-ROOT-018",
        SANDBOX_001 = "RPMX-SANDBOX-001",
        SANDBOX_002 = "RPMX-SANDBOX-002",
        SANDBOX_003 = "RPMX-SANDBOX-003",
        SANDBOX_004 = "RPMX-SANDBOX-004",
        SBOM_001 = "RPMX-SBOM-001",
        ASSEMBLY_012 = "RPMX-ASSEMBLY-012",
        ASSEMBLY_013 = "RPMX-ASSEMBLY-013",
        ASSEMBLY_014 = "RPMX-ASSEMBLY-014",
        ASSEMBLY_015 = "RPMX-ASSEMBLY-015",
        ASSEMBLY_016 = "RPMX-ASSEMBLY-016",
        ASSEMBLY_020 = "RPMX-ASSEMBLY-020",
        BUILD_001 = "RPMX-BUILD-001",
        CACHE_001 = "RPMX-CACHE-001",
        CACHE_002 = "RPMX-CACHE-002",
        BUILD_002 = "RPMX-BUILD-002",
        BUILD_003 = "RPMX-BUILD-003",
        CLI_001 = "RPMX-CLI-001",
        CLI_002 = "RPMX-CLI-002",
        CLI_003 = "RPMX-CLI-003",
        CLI_004 = "RPMX-CLI-004",
        IO_001 = "RPMX-IO-001",
        MATRIX_001 = "RPMX-MATRIX-001",
        MATRIX_002 = "RPMX-MATRIX-002",
        MATRIX_003 = "RPMX-MATRIX-003",
        MATRIX_004 = "RPMX-MATRIX-004",
        MATRIX_005 = "RPMX-MATRIX-005",
        MATRIX_006 = "RPMX-MATRIX-006",
        MATRIX_007 = "RPMX-MATRIX-007",
        MATRIX_008 = "RPMX-MATRIX-008",
        MATRIX_009 = "RPMX-MATRIX-009",
        MATRIX_010 = "RPMX-MATRIX-010",
        MATRIX_011 = "RPMX-MATRIX-011",
        OUTPUT_001 = "RPMX-OUTPUT-001",
        OUTPUT_002 = "RPMX-OUTPUT-002",
        OUTPUT_003 = "RPMX-OUTPUT-003",
        OUTPUT_004 = "RPMX-OUTPUT-004",
        OUTPUT_005 = "RPMX-OUTPUT-005",
        OUTPUT_010 = "RPMX-OUTPUT-010",
        OUTPUT_011 = "RPMX-OUTPUT-011",
        OUTPUT_012 = "RPMX-OUTPUT-012",
        OUTPUT_013 = "RPMX-OUTPUT-013",
        OUTPUT_014 = "RPMX-OUTPUT-014",
        OUTPUT_015 = "RPMX-OUTPUT-015",
        OUTPUT_016 = "RPMX-OUTPUT-016",
        OUTPUT_017 = "RPMX-OUTPUT-017",
        OUTPUT_018 = "RPMX-OUTPUT-018",
        PLAN_001 = "RPMX-PLAN-001",
        PLAN_002 = "RPMX-PLAN-002",
        PLAN_003 = "RPMX-PLAN-003",
        PLAN_004 = "RPMX-PLAN-004",
        PLAN_005 = "RPMX-PLAN-005",
        PLAN_006 = "RPMX-PLAN-006",
        PLAN_007 = "RPMX-PLAN-007",
        PLAN_008 = "RPMX-PLAN-008",
        PLAN_009 = "RPMX-PLAN-009",
        PLAN_010 = "RPMX-PLAN-010",
        PLAN_011 = "RPMX-PLAN-011",
        PLAN_012 = "RPMX-PLAN-012",
        RPM_001 = "RPMX-RPM-001",
        RPM_002 = "RPMX-RPM-002",
        RPM_003 = "RPMX-RPM-003",
        RPM_004 = "RPMX-RPM-004",
        RPM_011 = "RPMX-RPM-011",
        RPM_012 = "RPMX-RPM-012",
        RPM_013 = "RPMX-RPM-013",
        SCHEMA_001 = "RPMX-SCHEMA-001",
        SCHEMA_002 = "RPMX-SCHEMA-002",
        SIGN_001 = "RPMX-SIGN-001",
        SIGN_002 = "RPMX-SIGN-002",
        SIGN_003 = "RPMX-SIGN-003",
        SIGN_004 = "RPMX-SIGN-004",
        SIGN_005 = "RPMX-SIGN-005",
        SIGN_006 = "RPMX-SIGN-006",
        SOURCE_001 = "RPMX-SOURCE-001",
        SOURCE_002 = "RPMX-SOURCE-002",
        SOURCE_010 = "RPMX-SOURCE-010",
        SOURCE_011 = "RPMX-SOURCE-011",
        SOURCE_012 = "RPMX-SOURCE-012",
        SOURCE_013 = "RPMX-SOURCE-013",
        SOURCE_014 = "RPMX-SOURCE-014",
        SOURCE_015 = "RPMX-SOURCE-015",
        SOURCE_016 = "RPMX-SOURCE-016",
        SOURCE_017 = "RPMX-SOURCE-017",
        SOURCE_018 = "RPMX-SOURCE-018",
        SOURCE_020 = "RPMX-SOURCE-020",
        SRPM_001 = "RPMX-SRPM-001",
        SRPM_002 = "RPMX-SRPM-002",
        SRPM_003 = "RPMX-SRPM-003",
        SRPM_004 = "RPMX-SRPM-004",
        SRPM_005 = "RPMX-SRPM-005",
        SRPM_006 = "RPMX-SRPM-006",
        SRPM_007 = "RPMX-SRPM-007",
        SRPM_009 = "RPMX-SRPM-009",
        SRPM_010 = "RPMX-SRPM-010",
        SRPM_011 = "RPMX-SRPM-011",
        SRPM_012 = "RPMX-SRPM-012",
        STAGE_001 = "RPMX-STAGE-001",
        STAGE_002 = "RPMX-STAGE-002",
        STAGE_003 = "RPMX-STAGE-003",
        STAGE_004 = "RPMX-STAGE-004",
        STAGE_005 = "RPMX-STAGE-005",
        STAGE_006 = "RPMX-STAGE-006",
        STAGE_007 = "RPMX-STAGE-007",
        STEP_001 = "RPMX-STEP-001",
        STEP_002 = "RPMX-STEP-002",
        STEP_003 = "RPMX-STEP-003",
        STEP_010 = "RPMX-STEP-010",
        STEP_011 = "RPMX-STEP-011",
        STEP_012 = "RPMX-STEP-012",
        STEP_013 = "RPMX-STEP-013",
        STEP_014 = "RPMX-STEP-014",
        STEP_015 = "RPMX-STEP-015",
        STEP_016 = "RPMX-STEP-016",
        STEP_017 = "RPMX-STEP-017",
        STEP_018 = "RPMX-STEP-018",
        TEMPLATE_001 = "RPMX-TEMPLATE-001",
        TEMPLATE_002 = "RPMX-TEMPLATE-002",
        TEMPLATE_003 = "RPMX-TEMPLATE-003",
        TEMPLATE_004 = "RPMX-TEMPLATE-004",
        TEMPLATE_005 = "RPMX-TEMPLATE-005",
        TEMPLATE_006 = "RPMX-TEMPLATE-006",
        TEMPLATE_007 = "RPMX-TEMPLATE-007",
        TEST_001 = "RPMX-TEST-001",
        TEST_002 = "RPMX-TEST-002",
        TOOL_001 = "RPMX-TOOL-001",
        TOOL_002 = "RPMX-TOOL-002",
        TOOL_003 = "RPMX-TOOL-003",
        TOOL_004 = "RPMX-TOOL-004",
        TOOL_005 = "RPMX-TOOL-005",
        TOOL_006 = "RPMX-TOOL-006",
        YAML_001 = "RPMX-YAML-001",
        YAML_002 = "RPMX-YAML-002",
        YAML_003 = "RPMX-YAML-003",
        YAML_004 = "RPMX-YAML-004",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: Code,
    pub message: String,
    pub path: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl Error {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            path: None,
            line: None,
            column: None,
        }
    }

    pub fn at(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn with_location(mut self, line: Option<usize>, column: Option<usize>) -> Self {
        self.line = line;
        self.column = column;
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if let Some(path) = &self.path {
            write!(f, " at {path}")?;
        }
        if let Some(line) = self.line {
            write!(f, " (line {line}")?;
            if let Some(column) = self.column {
                write!(f, ", column {column}")?;
            }
            write!(f, ")")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::codes;

    #[test]
    fn every_code_is_unique_and_well_formed() {
        let mut seen = std::collections::BTreeSet::new();
        for code in codes::ALL {
            let text = code.as_str();
            assert!(seen.insert(text), "duplicate diagnostic code {text}");
            let rest = text
                .strip_prefix("RPMX-")
                .unwrap_or_else(|| panic!("code {text} lacks RPMX- prefix"));
            let (domain, number) = rest
                .rsplit_once('-')
                .unwrap_or_else(|| panic!("code {text} lacks a number"));
            assert!(
                !domain.is_empty() && domain.chars().all(|ch| ch.is_ascii_uppercase()),
                "code {text} has an invalid domain"
            );
            assert!(
                number.len() == 3 && number.chars().all(|ch| ch.is_ascii_digit()),
                "code {text} has an invalid number"
            );
        }
    }
}
