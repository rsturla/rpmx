use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

fn scalar_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::String(s) => Ok(s),
        Value::Number(n) => Ok(n.to_string()),
        other => Err(serde::de::Error::custom(format!(
            "expected a string or number, got {}",
            kind(&other)
        ))),
    }
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "list",
        Value::Object(_) => "map",
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema: u32,
    pub package: Package,
    /// A local directory copied into the source workspace before assembly.
    /// Paths are relative to the recipe directory and the contents land at the
    /// workspace root, like melange's `--source-dir`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default)]
    pub vars: BTreeMap<String, Value>,
    #[serde(default)]
    pub build: Build,
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub patches: Vec<Patch>,
    #[serde(default, rename = "source-assembly")]
    pub source_assembly: SourceAssembly,
    #[serde(default)]
    pub matrix: Option<Matrix>,
    #[serde(default)]
    pub outputs: Vec<Output>,
    #[serde(default)]
    pub tests: Vec<Test>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub name: String,
    #[serde(default)]
    pub epoch: Option<u32>,
    #[serde(deserialize_with = "scalar_string")]
    pub version: String,
    #[serde(deserialize_with = "scalar_string")]
    pub release: String,
    pub summary: String,
    #[serde(default)]
    pub description: Option<String>,
    pub license: String,
    /// Deterministic build timestamp in Unix seconds. It drives file mtimes,
    /// SBOM dates and the signature creation time, so two builds of the same
    /// source produce identical bytes.
    #[serde(default, rename = "source-date-epoch", alias = "source_date_epoch")]
    pub source_date_epoch: Option<u32>,
    #[serde(default)]
    pub upstream: Option<String>,
    #[serde(default)]
    pub vcs: Option<Vcs>,
    #[serde(default)]
    pub maintainers: Vec<Maintainer>,
    #[serde(default)]
    pub cpe: Option<Cpe>,
}

/// CPE 2.3 identity for vulnerability matching. `version` and `update` come
/// from the package at build time; everything else is recipe data.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cpe {
    #[serde(default = "default_cpe_part")]
    pub part: String,
    pub vendor: String,
    pub product: String,
    #[serde(default)]
    pub edition: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default, rename = "sw-edition")]
    pub sw_edition: Option<String>,
    #[serde(default, rename = "target-sw")]
    pub target_sw: Option<String>,
    #[serde(default, rename = "target-hw")]
    pub target_hw: Option<String>,
    #[serde(default)]
    pub other: Option<String>,
}

fn default_cpe_part() -> String {
    "a".to_string()
}

impl Cpe {
    pub fn cpe23(&self, version: &str, release: &str) -> String {
        format!(
            "cpe:2.3:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
            escape(&self.part),
            escape(&self.vendor),
            escape(&self.product),
            escape(version),
            escape(release),
            escape(self.edition.as_deref().unwrap_or("*")),
            escape(self.language.as_deref().unwrap_or("*")),
            escape(self.sw_edition.as_deref().unwrap_or("*")),
            escape(self.target_sw.as_deref().unwrap_or("*")),
            escape(self.target_hw.as_deref().unwrap_or("*")),
            escape(self.other.as_deref().unwrap_or("*")),
        )
    }
}

/// CPE 2.3 formatted-string escaping. Alphanumerics, `_`, `-` and `.` are
/// literal; wildcards and separators are quoted with a backslash.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | '*' | '?') {
            out.push(character);
        } else {
            out.push('\\');
            out.push(character);
        }
    }
    out
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Vcs {
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Maintainer {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    #[serde(default)]
    pub pipeline: Vec<Step>,
    #[serde(default)]
    pub dependencies: Dependencies,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// Build root source. Either a named profile or an inline environment.
    /// When absent, `--profile` supplies the root.
    #[serde(default)]
    pub root: Option<RootSpec>,
}

/// Where the build root comes from.
///
/// `profile` is a name or path; the remaining fields describe a root inline,
/// so a single recipe can build without a separate profile file.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootSpec {
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub releasever: Option<String>,
    #[serde(default)]
    pub archs: Vec<String>,
    #[serde(default)]
    pub repositories: Vec<RootRepository>,
    /// Packages to install in the root before the recipe's dependencies.
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default)]
    pub builder: Option<RootBuilder>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootRepository {
    pub id: String,
    #[serde(default)]
    pub baseurl: Option<String>,
    #[serde(default)]
    pub metalink: Option<String>,
    #[serde(default)]
    pub mirrorlist: Option<String>,
    #[serde(default)]
    pub gpgkey: Option<String>,
    /// Verify signed repository metadata in addition to package signatures.
    #[serde(default, rename = "repo-gpgcheck")]
    pub repo_gpgcheck: bool,
}

/// How the resolver authenticates to a repository.
///
/// Path-valued fields accept an `env:NAME` reference or an absolute file path.
/// Password fields must be an `env:NAME` reference or a file path, so a profile
/// cannot carry a literal secret. User names and proxy URLs may be literals.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryAuth {
    /// CA bundle used to verify the repository's TLS certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca: Option<String>,
    /// PEM client certificate for mutual TLS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert: Option<String>,
    /// PEM private key for the client certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Basic-auth user name. May be a literal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Basic-auth password. Must be an `env:` reference or a file path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Proxy URL. May be a literal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// Proxy user name. May be a literal.
    #[serde(
        default,
        rename = "proxy-username",
        skip_serializing_if = "Option::is_none"
    )]
    pub proxy_username: Option<String>,
    /// Proxy password. Must be an `env:` reference or a file path.
    #[serde(
        default,
        rename = "proxy-password",
        skip_serializing_if = "Option::is_none"
    )]
    pub proxy_password: Option<String>,
}

impl RepositoryAuth {
    pub fn is_empty(&self) -> bool {
        self.ca.is_none()
            && self.cert.is_none()
            && self.key.is_none()
            && self.username.is_none()
            && self.password.is_none()
            && self.proxy.is_none()
            && self.proxy_username.is_none()
            && self.proxy_password.is_none()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootBuilder {
    pub image: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Dependencies {
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub host: Vec<String>,
    #[serde(default)]
    pub target: Vec<String>,
    #[serde(default)]
    pub tests: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub uses: Option<String>,
    #[serde(default, rename = "with")]
    pub with: Option<BTreeMap<String, Value>>,
    #[serde(default)]
    pub argv: Option<Vec<String>>,
    #[serde(default)]
    pub runs: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub timeout: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

impl Step {
    pub fn label(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.uses.clone())
            .or_else(|| self.argv.as_ref().and_then(|a| a.first().cloned()))
            .unwrap_or_else(|| "<anonymous>".to_string())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub file: String,
    #[serde(default = "default_patch_strip")]
    pub strip: u32,
    #[serde(default)]
    pub reason: Option<String>,
}

fn default_patch_strip() -> u32 {
    1
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub sum: Option<String>,
    pub digest: String,
    #[serde(default)]
    pub locators: Vec<Locator>,
    /// How to unpack an archive into the source workspace.
    #[serde(default)]
    pub extract: Option<Extract>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Extract {
    /// `auto`, `tar`, `tar.gz`, `tar.xz`, `tar.zst` or `none`.
    #[serde(default)]
    pub format: Option<String>,
    /// Path components to strip. Defaults to 1, matching common tarballs.
    #[serde(default, rename = "strip-components")]
    pub strip_components: Option<u32>,
    /// Directory under the workspace to unpack into.
    #[serde(default)]
    pub destination: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Locator {
    pub kind: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default)]
    pub sumdb: Option<String>,
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceAssembly {
    #[serde(default)]
    pub pipeline: Vec<Step>,
    #[serde(default)]
    pub tools: Vec<Tool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub name: String,
    pub version: String,
    pub digest: String,
    #[serde(default)]
    pub locators: Vec<Locator>,
    #[serde(default)]
    pub entrypoint: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Matrix {
    pub axes: BTreeMap<String, Vec<Value>>,
    #[serde(default)]
    pub exclude: Vec<BTreeMap<String, Value>>,
    #[serde(default, rename = "max-cells")]
    pub max_cells: Option<usize>,
    #[serde(default, rename = "release-suffix")]
    pub release_suffix: Option<bool>,
}

/// One explicitly declared bundled component for an output.
///
/// Explicit declarations are always generic. Ecosystems with machine-readable
/// vendor metadata are discovered by catalogers instead; a recipe cannot forge
/// `golang(...)` or `crate(...)` evidence here.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BundledDeclaration {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    /// SPDX license expression for the SBOM. It does not change the provide.
    #[serde(default)]
    pub license: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub name: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub metapackage: bool,
    #[serde(default)]
    pub files: Files,
    #[serde(default)]
    pub dependencies: OutputDependencies,
    #[serde(default)]
    pub bundled: Vec<BundledDeclaration>,
    #[serde(default)]
    pub generators: OutputGenerators,
    #[serde(default)]
    pub lint: OutputLint,
    #[serde(default)]
    pub scriptlets: Scriptlets,
    /// Tests for this output. An installed-stage test owned by an output is
    /// prepared with that output and its runtime dependencies, not with every
    /// sibling output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<Test>,
}

/// Per-output lint policy declared by the recipe.
///
/// `allow` is a list of generic lint rule names that this output intentionally
/// violates. It exists for outputs that legitimately carry files the default
/// policy would otherwise reject; it is not a general escape hatch.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputLint {
    #[serde(default)]
    pub allow: Vec<String>,
}

/// Per-output switches for automatic `Provides`/`Requires` generation.
///
/// Every generator is on by default. `exclude` removes a generated dependency
/// (provide or require) by exact match, which is the escape hatch for a
/// generator that is right in general and wrong here.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputGenerators {
    /// ELF sonames, versioned symbols, and `.so` symlink requirements.
    #[serde(default = "enabled")]
    pub elf: bool,
    /// `pkgconfig(...)` provides and requires.
    #[serde(default = "enabled")]
    pub pkgconfig: bool,
    /// Interpreter requirements from executable `#!` lines.
    #[serde(default = "enabled")]
    pub script: bool,
    /// `python(abi)` and `pythonX.Ydist(...)` dependencies.
    #[serde(default = "enabled")]
    pub python: bool,
    /// `perl(...)` provides and requires.
    #[serde(default = "enabled")]
    pub perl: bool,
    /// `application(...)` and `mimehandler(...)` provides.
    #[serde(default = "enabled")]
    pub desktop: bool,
    /// `metainfo(...)` provides.
    #[serde(default = "enabled")]
    pub metainfo: bool,
    /// `debuginfo(build-id)` provides.
    #[serde(default = "enabled")]
    pub debuginfo: bool,
    /// `bundled(...)` provides from the component inventory.
    #[serde(default = "enabled")]
    pub bundled: bool,
    /// Generated dependencies to drop, by exact string.
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl Default for OutputGenerators {
    fn default() -> Self {
        Self {
            elf: true,
            pkgconfig: true,
            script: true,
            python: true,
            perl: true,
            desktop: true,
            metainfo: true,
            debuginfo: true,
            bundled: true,
            exclude: Vec::new(),
        }
    }
}

fn enabled() -> bool {
    true
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scriptlets {
    /// Interpreter for every scriptlet; defaults to `/bin/sh`.
    #[serde(default)]
    pub interpreter: Option<String>,
    #[serde(default)]
    pub pre: Option<String>,
    #[serde(default)]
    pub post: Option<String>,
    #[serde(default)]
    pub preun: Option<String>,
    #[serde(default)]
    pub postun: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Files {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub license: Vec<String>,
    #[serde(default)]
    pub doc: Vec<String>,
    #[serde(default)]
    pub config: Vec<String>,
    #[serde(default)]
    pub noreplace: Vec<String>,
    /// Explicit directory entries, including empty directories.
    #[serde(default)]
    pub directories: Vec<Directory>,
    /// Per-path overrides for regular files and symlinks.
    #[serde(default)]
    pub attributes: Vec<Attribute>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Directory {
    pub path: String,
    /// Octal string, for example `"0750"`.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attribute {
    pub path: String,
    /// Octal string, for example `"0755"`.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    /// RPM file capabilities, for example `cap_net_raw=ep`.
    #[serde(default)]
    pub caps: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DependencySet {
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub recommends: Vec<String>,
    #[serde(default)]
    pub suggests: Vec<String>,
    #[serde(default)]
    pub supplements: Vec<String>,
    #[serde(default)]
    pub enhances: Vec<String>,
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(default)]
    pub obsoletes: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputDependencies {
    #[serde(default)]
    pub run: Vec<String>,
    #[serde(default)]
    pub recommends: Vec<String>,
    #[serde(default)]
    pub suggests: Vec<String>,
    #[serde(default)]
    pub supplements: Vec<String>,
    #[serde(default)]
    pub enhances: Vec<String>,
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(default)]
    pub obsoletes: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
}

impl OutputDependencies {
    pub fn to_set(&self) -> DependencySet {
        DependencySet {
            requires: self.run.clone(),
            recommends: self.recommends.clone(),
            suggests: self.suggests.clone(),
            supplements: self.supplements.clone(),
            enhances: self.enhances.clone(),
            provides: self.provides.clone(),
            obsoletes: self.obsoletes.clone(),
            conflicts: self.conflicts.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Test {
    pub name: String,
    pub stage: String,
    #[serde(default)]
    pub argv: Option<Vec<String>>,
    #[serde(default)]
    pub pipeline: Option<Vec<Step>>,
    /// Test-only root and environment, analogous to a Wolfi/melange test
    /// environment. Installed-stage tests run in a disposable root; the
    /// packages here are added to that root.
    #[serde(default)]
    pub environment: TestEnvironment,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestEnvironment {
    #[serde(default)]
    pub contents: TestContents,
    /// Variables applied to every step of this test. A step's own `env`
    /// entries win.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestContents {
    /// Packages installed only for this test.
    #[serde(default)]
    pub packages: Vec<String>,
}

/// One YAML file in a recipe bundle.
///
/// The root recipe and every included fragment use this shape. A fragment may
/// omit `schema`; it must not declare one. Every section may appear in only
/// one file across the bundle.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeFragment {
    #[serde(default)]
    pub schema: Option<u32>,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub package: Option<Package>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub vars: Option<BTreeMap<String, Value>>,
    #[serde(default)]
    pub build: Option<Build>,
    #[serde(default)]
    pub sources: Option<Vec<Source>>,
    #[serde(default)]
    pub patches: Option<Vec<Patch>>,
    #[serde(default, rename = "source-assembly")]
    pub source_assembly: Option<SourceAssembly>,
    #[serde(default)]
    pub matrix: Option<Matrix>,
    #[serde(default)]
    pub outputs: Option<Vec<Output>>,
    #[serde(default)]
    pub tests: Option<Vec<Test>>,
}
