//! Process exit codes.
//!
//! The taxonomy is part of the CLI contract: a caller must be able to tell a
//! recipe mistake from a build failure without parsing the message. Codes are
//! derived from the diagnostic domain, with a few exact overrides where one
//! domain serves several stages.

use rpmx_core::error::{codes, Code};

pub const RECIPE: u8 = 2;
pub const LOCK_DRIFT: u8 = 3;
pub const RESOLUTION: u8 = 4;
pub const BUILD: u8 = 5;
pub const TEST: u8 = 6;
pub const POLICY: u8 = 7;
pub const EVIDENCE: u8 = 8;
pub const INTERNAL: u8 = 9;

/// Return the process exit code for a diagnostic.
pub fn code_for(code: Code) -> u8 {
    if code == codes::CLI_001 || code == codes::CLI_007 {
        // Invalid command-line value, such as --arch, --arch-jobs or --source.
        return RECIPE;
    }
    if code == codes::BUILD_007 {
        // Installed-test setup or execution failed.
        return TEST;
    }
    if code == codes::BOOTSTRAP_005 {
        // The ladder built, but its fixed point did not converge.
        return BUILD;
    }
    if code == codes::BOOTSTRAP_006 {
        // A stage lock is missing or has drifted.
        return LOCK_DRIFT;
    }
    if code == codes::SRPM_011
        || code == codes::SRPM_012
        || code == codes::ROOT_013
        || code == codes::ROOT_017
    {
        // A rebuild or root disagrees with its lock.
        return LOCK_DRIFT;
    }
    match domain(code) {
        // Prerequisite and recipe errors, not test failures.
        _ if code == codes::LOCK_003 || code == codes::TEST_004 || code == codes::TEST_006 => {
            RECIPE
        }
        "LOCK" => LOCK_DRIFT,
        "ROOT" | "FETCH" => RESOLUTION,
        "BUILD" | "SANDBOX" | "RUNNER" | "RPM" | "SRPM" => BUILD,
        "TEST" => TEST,
        "POLICY" | "SIGN" => POLICY,
        "SBOM" | "EXTRACT" => EVIDENCE,
        "YAML" | "SCHEMA" | "PLAN" | "OUTPUT" | "STEP" | "TEMPLATE" | "MATRIX" | "TOOL"
        | "INCLUDE" | "ASSEMBLY" | "SOURCE" | "STAGE" | "LINT" | "INIT" | "BUNDLED"
        | "BOOTSTRAP" => RECIPE,
        _ => INTERNAL,
    }
}

fn domain(code: Code) -> &'static str {
    code.as_str()
        .strip_prefix("RPMX-")
        .and_then(|rest| rest.rsplit_once('-').map(|(domain, _)| domain))
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::code_for;
    use rpmx_core::error::codes;

    // Published numbers, not the constants: changing the taxonomy must fail
    // here as well as in the CLI contract.
    #[test]
    fn domains_map_to_stable_exit_codes() {
        assert_eq!(code_for(codes::YAML_001), 2);
        assert_eq!(code_for(codes::OUTPUT_011), 2);
        assert_eq!(code_for(codes::ASSEMBLY_016), 2);
        assert_eq!(code_for(codes::ROOT_007), 4);
        assert_eq!(code_for(codes::FETCH_004), 4);
        assert_eq!(code_for(codes::SANDBOX_002), 5);
        assert_eq!(code_for(codes::RUNNER_002), 5);
        assert_eq!(code_for(codes::TEST_001), 6);
        assert_eq!(code_for(codes::TEST_003), 6);
        assert_eq!(code_for(codes::SIGN_002), 7);
        assert_eq!(code_for(codes::SBOM_001), 8);
        assert_eq!(code_for(codes::IO_001), 9);
    }

    #[test]
    fn stage_specific_overrides_win_over_the_domain() {
        assert_eq!(code_for(codes::CLI_001), 2);
        assert_eq!(code_for(codes::CLI_007), 2);
        assert_eq!(code_for(codes::BUNDLED_003), 2);
        assert_eq!(code_for(codes::BOOTSTRAP_001), 2);
        assert_eq!(code_for(codes::BOOTSTRAP_004), 2);
        assert_eq!(code_for(codes::BOOTSTRAP_007), 2);
        assert_eq!(code_for(codes::BOOTSTRAP_008), 2);
        assert_eq!(code_for(codes::BOOTSTRAP_009), 2);
        assert_eq!(code_for(codes::BOOTSTRAP_005), 5);
        assert_eq!(code_for(codes::BOOTSTRAP_006), 3);
        assert_eq!(code_for(codes::BUILD_007), 6);
        assert_eq!(code_for(codes::SRPM_011), 3);
        assert_eq!(code_for(codes::SRPM_012), 3);
        assert_eq!(code_for(codes::ROOT_013), 3);
        assert_eq!(code_for(codes::ROOT_017), 3);
    }
}
