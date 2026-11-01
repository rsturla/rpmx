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
    if code == codes::BUILD_007 {
        // Installed-test setup or execution failed.
        return TEST;
    }
    if code == codes::SRPM_011 || code == codes::SRPM_012 {
        // A rebuild disagrees with the lock recorded in the SRPM.
        return LOCK_DRIFT;
    }
    match domain(code) {
        "LOCK" => LOCK_DRIFT,
        "ROOT" | "FETCH" => RESOLUTION,
        "BUILD" | "SANDBOX" | "RUNNER" | "RPM" | "SRPM" => BUILD,
        "TEST" => TEST,
        "POLICY" | "SIGN" => POLICY,
        "SBOM" | "EXTRACT" => EVIDENCE,
        "YAML" | "SCHEMA" | "PLAN" | "OUTPUT" | "STEP" | "TEMPLATE" | "MATRIX" | "TOOL"
        | "SOURCE" | "STAGE" => RECIPE,
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
    use super::{
        code_for, BUILD, EVIDENCE, INTERNAL, LOCK_DRIFT, POLICY, RECIPE, RESOLUTION, TEST,
    };
    use rpmx_core::error::codes;

    #[test]
    fn domains_map_to_stable_exit_codes() {
        assert_eq!(code_for(codes::YAML_001), RECIPE);
        assert_eq!(code_for(codes::OUTPUT_011), RECIPE);
        assert_eq!(code_for(codes::ROOT_007), RESOLUTION);
        assert_eq!(code_for(codes::FETCH_004), RESOLUTION);
        assert_eq!(code_for(codes::SANDBOX_002), BUILD);
        assert_eq!(code_for(codes::RUNNER_002), BUILD);
        assert_eq!(code_for(codes::TEST_001), TEST);
        assert_eq!(code_for(codes::SIGN_002), POLICY);
        assert_eq!(code_for(codes::SBOM_001), EVIDENCE);
        assert_eq!(code_for(codes::IO_001), INTERNAL);
    }

    #[test]
    fn stage_specific_overrides_win_over_the_domain() {
        assert_eq!(code_for(codes::BUILD_007), TEST);
        assert_eq!(code_for(codes::SRPM_011), LOCK_DRIFT);
        assert_eq!(code_for(codes::SRPM_012), LOCK_DRIFT);
    }
}
