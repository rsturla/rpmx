use std::path::Path;

use crate::commands::PlanPolicy;

pub struct SrpmArgs<'a> {
    pub file: &'a Path,
    pub sources: &'a Path,
    pub output: &'a Path,
    pub arch: &'a str,
    pub source_date_epoch: Option<u32>,
    pub cell: Option<&'a str>,
    pub pipeline_dir: Option<&'a Path>,
    pub policy: PlanPolicy<'a>,
    pub root_lock: Option<&'a Path>,
    pub signing_key: Option<&'a Path>,
    pub signing_passphrase_env: Option<&'a str>,
    pub signing_timestamp: Option<u32>,
    pub quiet: bool,
    pub json: bool,
}

pub struct BuildArgs<'a> {
    pub file: &'a Path,
    pub stage: Option<&'a Path>,
    pub output: &'a Path,
    pub arch: &'a str,
    pub runner: &'a str,
    pub source_date_epoch: Option<u32>,
    pub cell: Option<&'a str>,
    pub pipeline_dir: Option<&'a Path>,
    pub policy: PlanPolicy<'a>,
    pub cache: Option<&'a Path>,
    pub proxy: Option<&'a str>,
    pub offline: bool,
    pub root_lock: Option<&'a Path>,
    pub root: Option<&'a Path>,
    pub unsafe_host_root: bool,
    pub jobs: u32,
    pub step_timeout: u64,
    pub run_tests: bool,
    pub sbom: Option<&'a Path>,
    pub test_root_lock: Option<&'a Path>,
    pub signing_key: Option<&'a Path>,
    pub signing_passphrase_env: Option<&'a str>,
    pub signing_timestamp: Option<u32>,
    pub json: bool,
}

pub struct RebuildArgs<'a> {
    pub file: &'a Path,
    pub output: &'a Path,
    pub arch: &'a str,
    pub runner: &'a str,
    pub cell: Option<&'a str>,
    pub policy: PlanPolicy<'a>,
    pub jobs: u32,
    pub step_timeout: u64,
    pub root_image: Option<&'a str>,
    pub root_lock: Option<&'a Path>,
    pub root: Option<&'a Path>,
    pub run_tests: bool,
    pub sbom: Option<&'a Path>,
    pub test_root_lock: Option<&'a Path>,
    pub signing_key: Option<&'a Path>,
    pub signing_passphrase_env: Option<&'a str>,
    pub signing_timestamp: Option<u32>,
    pub quiet: bool,
    pub json: bool,
}

pub struct FetchArgs<'a> {
    pub file: &'a Path,
    pub cache: Option<&'a Path>,
    pub proxy: Option<&'a str>,
    pub offline: bool,
    pub json: bool,
}

pub struct SourceArgs<'a> {
    pub file: &'a Path,
    pub pipeline_dir: Option<&'a Path>,
    pub policy: PlanPolicy<'a>,
    pub cache: Option<&'a Path>,
    pub proxy: Option<&'a str>,
    pub offline: bool,
    pub output: &'a Path,
    pub arch: &'a str,
    pub runner: &'a str,
    pub source_date_epoch: Option<u32>,
    pub jobs: u32,
    pub step_timeout: u64,
    pub root_lock: Option<&'a Path>,
    pub root: Option<&'a Path>,
    pub unsafe_host_root: bool,
    pub quiet: bool,
    pub json: bool,
}

pub struct RootResolveArgs<'a> {
    pub profile: &'a Path,
    pub output: Option<&'a Path>,
    pub work: Option<&'a Path>,
    pub keep: bool,
    pub cache: Option<&'a Path>,
    pub json: bool,
}

pub struct RootAssembleArgs<'a> {
    pub lock: &'a Path,
    pub root: Option<&'a Path>,
    pub output: Option<&'a Path>,
    pub cache: Option<&'a Path>,
    pub work: Option<&'a Path>,
    pub keep: bool,
    pub json: bool,
}

pub struct RootDigestArgs<'a> {
    pub root: &'a Path,
    pub json: bool,
}

pub struct RootExportArgs<'a> {
    pub lock: &'a Path,
    pub output: Option<&'a Path>,
}

pub struct RootImportArgs<'a> {
    pub manifest: &'a Path,
    pub profile: &'a str,
    pub arch: Option<&'a str>,
    pub releasever: &'a str,
    pub output: Option<&'a Path>,
}
