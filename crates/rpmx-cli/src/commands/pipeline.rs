use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};

use rpmx_core::error::codes;
use rpmx_core::plan::PlannedCell;
use rpmx_core::Error;

use crate::commands::{
    rebuild::{self, RebuildRequest},
    source::{self, SourceRequest},
    srpm::{self, SrpmRequest},
    OptionalSigningArgs, PlanPolicy,
};

/// Shared inputs for the SRPM-first build. The stage commands implement the
/// same steps, but this API lets a full build run them without rebuilding
/// clap argument structs.
pub struct BuildPipeline<'a> {
    pub file: &'a Path,
    pub output: &'a Path,
    pub arch: &'a str,
    /// Where steps execute: the host for a cross build, the target otherwise.
    pub exec: rpmx_core::arch::Arch,
    /// Target sysroot, mounted read-only, for a cross build.
    pub sysroot: Option<&'a Path>,
    /// Toolchain commands for a cross build.
    pub cross: Option<&'a rpmx_core::guest::CrossToolchain>,
    /// Package names in the target sysroot, which satisfy host dependencies.
    pub sysroot_packages: Option<&'a std::collections::BTreeSet<String>>,
    /// Target sysroot lock, embedded in each SRPM for a cross build.
    pub sysroot_lock: Option<&'a rpmx_core::root::RootLock>,
    pub runner: &'a str,
    pub policy: PlanPolicy<'a>,
    pub profile: Option<&'a Path>,
    pub cache: Option<&'a Path>,
    pub proxy: Option<&'a str>,
    pub offline: bool,
    pub root_lock: Option<&'a Path>,
    pub root: Option<&'a Path>,
    pub runner_asset_root: Option<&'a Path>,
    pub unsafe_host_root: bool,
    pub source_date_epoch: Option<u32>,
    pub jobs: NonZeroU32,
    pub step_timeout: NonZeroU64,
    pub run_tests: bool,
    pub test_filter: &'a [String],
    pub junit: Option<&'a Path>,
    pub no_lint: bool,
    pub sbom: Option<&'a Path>,
    pub test_root_lock: Option<&'a Path>,
    pub signing: &'a OptionalSigningArgs,
    pub pipeline_dir: Option<&'a Path>,
}

pub struct PipelineOutput {
    pub rpms: Vec<PathBuf>,
    pub srpms: Vec<PathBuf>,
    pub evidence: Vec<PathBuf>,
}

impl BuildPipeline<'_> {
    /// Assemble sources, package one SRPM per cell, then rebuild each SRPM.
    pub fn execute(&self, cells: &[&PlannedCell]) -> Result<PipelineOutput, Error> {
        std::fs::create_dir_all(self.output).map_err(|error| {
            Error::new(
                codes::BUILD_001,
                format!("cannot create {}: {error}", self.output.display()),
            )
        })?;
        let work = crate::commands::WorkDir::new("build")?;
        let source_output = work.path().join("source");
        std::fs::create_dir_all(&source_output).map_err(|error| {
            Error::new(
                codes::BUILD_006,
                format!("cannot create {}: {error}", source_output.display()),
            )
        })?;
        let srpm_output = self.output.join("src");

        source::execute(SourceRequest {
            file: self.file,
            arch: self.arch,
            exec: self.exec,
            sysroot: self.sysroot,
            cross: self.cross,
            runner: self.runner,
            policy: self.policy,
            profile: self.profile,
            cache: self.cache,
            proxy: self.proxy,
            offline: self.offline,
            locked: false,
            root_lock: self.root_lock,
            root: self.root,
            runner_asset_root: self.runner_asset_root,
            unsafe_host_root: self.unsafe_host_root,
            verify_root: false,
            output: &source_output,
            source_date_epoch: self.source_date_epoch,
            jobs: self.jobs,
            step_timeout: self.step_timeout,
            quiet: true,
            pipeline_dir: self.pipeline_dir,
        })?;

        let mut output = PipelineOutput {
            rpms: Vec::new(),
            srpms: Vec::new(),
            evidence: Vec::new(),
        };
        for planned in cells {
            let written = srpm::execute(SrpmRequest {
                file: self.file,
                policy: self.policy,
                sources: &source_output,
                output: &srpm_output,
                arch: self.arch,
                source_date_epoch: self.source_date_epoch,
                cell: Some(&planned.id),
                root_lock: self.root_lock,
                exec: self.exec,
                sysroot_lock: self.sysroot_lock,
                cross: self.cross,
                signing: self.signing,
                quiet: true,
                pipeline_dir: self.pipeline_dir,
            })?;
            output.srpms.extend(written.iter().cloned());

            for srpm_path in written {
                let rebuilt = rebuild::execute(RebuildRequest {
                    file: &srpm_path,
                    output: self.output,
                    arch: self.arch,
                    exec: self.exec,
                    sysroot: self.sysroot,
                    cross: self.cross,
                    sysroot_packages: self.sysroot_packages,
                    runner: self.runner,
                    cell: Some(&planned.id),
                    policy: self.policy,
                    jobs: self.jobs,
                    step_timeout: self.step_timeout,
                    root_image: None,
                    root_lock: self.root_lock,
                    root: self.root,
                    runner_asset_root: self.runner_asset_root,
                    cache: self.cache,
                    offline: self.offline,
                    verify_root: false,
                    run_tests: self.run_tests,
                    test_filter: self.test_filter,
                    no_index: true,
                    no_lint: self.no_lint,
                    junit: self.junit,
                    sbom: self.sbom,
                    test_root_lock: self.test_root_lock,
                    signing: self.signing,
                    quiet: true,
                })?;
                output.rpms.extend(rebuilt.rpms);
                output.evidence.extend(rebuilt.evidence);
            }
        }
        Ok(output)
    }
}
