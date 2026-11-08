use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha512};

use crate::template;
use rpmx_base::error::{codes, Code, Error, Result};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepFile {
    pub name: String,
    pub version: u32,
    pub summary: String,
    #[serde(default)]
    pub inputs: BTreeMap<String, InputSpec>,
    #[serde(default)]
    pub needs: Needs,
    #[serde(default)]
    pub render: Option<String>,
    pub run: Run,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSpec {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Needs {
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub host: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    #[serde(default)]
    pub argv: Option<Vec<String>>,
    #[serde(default)]
    pub runs: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone)]
pub struct ResolvedStep {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    /// Build-root tools, from `needs.tools`.
    pub needs_tools: Vec<String>,
    /// Target libraries, from `needs.host`.
    pub needs_host: Vec<String>,
    /// Union of both, for callers that do not distinguish the two roots.
    pub needs: Vec<String>,
    pub digest: String,
    pub builtin: bool,
    pub input: Option<String>,
    pub input_file: Option<String>,
}

const BUILTINS: &[(&str, &str)] = &[
    ("go/bump", include_str!("../pipelines/go/bump.yaml")),
    ("go/build", include_str!("../pipelines/go/build.yaml")),
    ("go/test", include_str!("../pipelines/go/test.yaml")),
    ("go/vendor", include_str!("../pipelines/go/vendor.yaml")),
    (
        "cargo/vendor",
        include_str!("../pipelines/cargo/vendor.yaml"),
    ),
    (
        "bundled/declare",
        include_str!("../pipelines/bundled/declare.yaml"),
    ),
    ("npm/ci", include_str!("../pipelines/npm/ci.yaml")),
    ("cmake", include_str!("../pipelines/cmake.yaml")),
    (
        "cmake/configure",
        include_str!("../pipelines/cmake/configure.yaml"),
    ),
    ("cmake/build", include_str!("../pipelines/cmake/build.yaml")),
    ("cmake/check", include_str!("../pipelines/cmake/check.yaml")),
    (
        "cmake/install",
        include_str!("../pipelines/cmake/install.yaml"),
    ),
    ("meson", include_str!("../pipelines/meson.yaml")),
    ("meson/setup", include_str!("../pipelines/meson/setup.yaml")),
    (
        "meson/compile",
        include_str!("../pipelines/meson/compile.yaml"),
    ),
    ("meson/test", include_str!("../pipelines/meson/test.yaml")),
    (
        "meson/install",
        include_str!("../pipelines/meson/install.yaml"),
    ),
    ("autotools", include_str!("../pipelines/autotools.yaml")),
    (
        "autotools/configure",
        include_str!("../pipelines/autotools/configure.yaml"),
    ),
    (
        "autotools/build",
        include_str!("../pipelines/autotools/build.yaml"),
    ),
    (
        "autotools/check",
        include_str!("../pipelines/autotools/check.yaml"),
    ),
    (
        "autotools/install",
        include_str!("../pipelines/autotools/install.yaml"),
    ),
    ("strip", include_str!("../pipelines/strip.yaml")),
    (
        "license/copy",
        include_str!("../pipelines/license/copy.yaml"),
    ),
    ("debuginfo", include_str!("../pipelines/debuginfo.yaml")),
    ("make", include_str!("../pipelines/make.yaml")),
    ("make/build", include_str!("../pipelines/make/build.yaml")),
    ("make/check", include_str!("../pipelines/make/check.yaml")),
    (
        "make/install",
        include_str!("../pipelines/make/install.yaml"),
    ),
    ("python", include_str!("../pipelines/python.yaml")),
    (
        "python/build-wheel",
        include_str!("../pipelines/python/build-wheel.yaml"),
    ),
    (
        "python/install-wheel",
        include_str!("../pipelines/python/install-wheel.yaml"),
    ),
];

/// Every built-in step name, in declaration order.
pub fn builtin_names() -> impl Iterator<Item = &'static str> {
    BUILTINS.iter().map(|(name, _)| *name)
}

pub fn builtin_source(name: &str) -> Option<&'static str> {
    BUILTINS
        .iter()
        .find(|(builtin, _)| *builtin == name)
        .map(|(_, source)| *source)
}

pub fn local_path(root: Option<&Path>, name: &str) -> Option<std::path::PathBuf> {
    let root = root?;
    // Single-segment names are reserved for built-ins, but an SRPM may embed
    // its own copy under `pipelines/<name>.yaml`; rebuild has to resolve that
    // copy so the locked digest is what actually runs.
    if validate_name(name).is_err() && builtin_source(name).is_none() {
        return None;
    }
    Some(root.join(format!("{name}.yaml")))
}

/// The YAML text of one step, read and decoded in a single place.
pub struct StepSource {
    text: String,
}

impl StepSource {
    pub fn from_text(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    pub fn read(path: &Path, read_code: Code, utf8_code: Code) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|error| {
            Error::new(
                read_code,
                format!("cannot read {}: {error}", path.display()),
            )
        })?;
        let text = String::from_utf8(bytes).map_err(|error| {
            Error::new(
                utf8_code,
                format!("{} is not valid UTF-8: {error}", path.display()),
            )
        })?;
        Ok(Self { text })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn into_text(self) -> String {
        self.text
    }

    pub fn digest(&self) -> String {
        format!("sha512:{:x}", Sha512::digest(self.text.as_bytes()))
    }
}

pub fn resolve(
    name: &str,
    root: Option<&Path>,
    with: Option<&BTreeMap<String, Value>>,
    allow_builtin_override: bool,
) -> Result<ResolvedStep> {
    let local = local_path(root, name).filter(|path| path.exists());
    let builtin = builtin_source(name);

    let (source, builtin) = match (builtin, local) {
        (Some(_), Some(_)) if !allow_builtin_override => {
            return Err(Error::new(
                codes::STEP_016,
                format!("local step '{name}' would shadow a built-in step"),
            ));
        }
        (_, Some(path)) => (
            StepSource::read(&path, codes::STEP_011, codes::STEP_012)?,
            false,
        ),
        (Some(source), None) => (StepSource::from_text(source), true),
        (None, None) => {
            validate_name(name)?;
            let root = root.ok_or_else(|| {
                Error::new(
                    codes::STEP_011,
                    format!("unknown step '{name}' and no pipeline directory is configured"),
                )
            })?;
            let path = root.join(format!("{name}.yaml"));
            (
                StepSource::read(&path, codes::STEP_011, codes::STEP_012)?,
                false,
            )
        }
    };

    let digest = source.digest();
    let file: StepFile = serde_yaml_ng::from_str(source.text()).map_err(|error| {
        Error::new(
            codes::STEP_012,
            format!("step '{name}' is invalid: {error}"),
        )
    })?;
    if file.name != name {
        return Err(Error::new(
            codes::STEP_013,
            format!(
                "step file for '{name}' declares name '{}', expected '{name}'",
                file.name
            ),
        ));
    }
    validate_guest_paths(name, &file)?;

    let scope = bind_inputs(&file, with)?;
    let (command, args) = render_run(&file.run, &scope, name)?;
    let cwd = match &file.run.cwd {
        Some(cwd) => Some(template::expand_inputs(cwd, &scope)?),
        None => None,
    };
    let mut env = BTreeMap::new();
    for (key, value) in file.run.env.iter().flatten() {
        env.insert(key.clone(), template::expand_inputs(value, &scope)?);
    }

    let needs_tools = file.needs.tools.clone();
    let needs_host = file.needs.host.clone();
    let mut needs = needs_tools.clone();
    needs.extend(needs_host.iter().cloned());

    let (input, input_file) = match file.render.as_deref() {
        None => (None, None),
        Some("omnibump-deps") => {
            let deps = scope
                .get("inputs.deps")
                .ok_or_else(|| Error::new(codes::ASSEMBLY_011, "go/bump requires 'deps'"))?;
            (Some(render_omnibump(deps)?), Some("deps.yaml".to_string()))
        }
        Some(other) => {
            return Err(Error::new(
                codes::STEP_018,
                format!("unknown step renderer '{other}'"),
            ));
        }
    };

    Ok(ResolvedStep {
        name: file.name,
        command,
        args,
        cwd,
        env,
        needs_tools,
        needs_host,
        needs,
        digest,
        builtin,
        input,
        input_file,
    })
}

fn render_omnibump(deps: &Value) -> Result<String> {
    let deps = deps
        .as_array()
        .ok_or_else(|| Error::new(codes::ASSEMBLY_011, "go/bump 'deps' must be a list"))?;
    if deps.is_empty() {
        return Err(Error::new(
            codes::ASSEMBLY_011,
            "go/bump deps must not be empty",
        ));
    }

    #[derive(serde::Serialize)]
    struct Package {
        name: String,
        version: String,
    }
    #[derive(serde::Serialize)]
    struct Deps {
        packages: Vec<Package>,
    }

    let mut packages = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (index, dep) in deps.iter().enumerate() {
        let map = dep.as_object().ok_or_else(|| {
            Error::new(codes::ASSEMBLY_012, "each go/bump dep must be a map")
                .at(format!("deps[{index}]"))
        })?;
        let field = |name: &str| {
            map.get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
        };
        let module = field("module");
        let version = field("version");
        let reason = field("reason");
        if module.is_empty() || version.is_empty() {
            return Err(Error::new(
                codes::ASSEMBLY_012,
                "go/bump dep needs non-empty 'module' and 'version'",
            )
            .at(format!("deps[{index}].module")));
        }
        if matches!(version, "latest" | "@latest" | "*") || version.contains('*') {
            return Err(Error::new(
                codes::ASSEMBLY_014,
                format!("go/bump dep '{module}' must use an exact version"),
            )
            .at(format!("deps[{index}].version")));
        }
        if reason.is_empty() {
            return Err(Error::new(
                codes::ASSEMBLY_015,
                format!("go/bump dep '{module}' needs a reason"),
            )
            .at(format!("deps[{index}].reason")));
        }
        if !seen.insert(module.to_string()) {
            return Err(Error::new(
                codes::ASSEMBLY_013,
                format!("go/bump lists '{module}' twice"),
            )
            .at(format!("deps[{index}].module")));
        }
        packages.push(Package {
            name: module.to_string(),
            version: version.to_string(),
        });
    }

    serde_yaml_ng::to_string(&Deps { packages })
        .map_err(|error| Error::new(codes::ASSEMBLY_020, error.to_string()))
}

pub fn validate_name(name: &str) -> Result<()> {
    let mut parts = name.split('/');
    let (Some(namespace), Some(step), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(Error::new(
            codes::STEP_010,
            format!("step name '{name}' must be '<namespace>/<name>'"),
        ));
    };
    if !valid_segment(namespace) || !valid_segment(step) {
        return Err(Error::new(
            codes::STEP_010,
            format!("step name '{name}' may contain only letters, digits, '_' and '-'"),
        ));
    }
    Ok(())
}

fn valid_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        && !segment.starts_with('-')
        && !segment.starts_with('_')
}

fn bind_inputs(
    file: &StepFile,
    with: Option<&BTreeMap<String, Value>>,
) -> Result<BTreeMap<String, Value>> {
    let with = with.cloned().unwrap_or_default();
    for key in with.keys() {
        if !file.inputs.contains_key(key) {
            return Err(Error::new(
                codes::STEP_014,
                format!("step '{}' has no input '{key}'", file.name),
            ));
        }
    }

    let mut scope = BTreeMap::new();
    for (name, spec) in &file.inputs {
        let value = with
            .get(name)
            .or(spec.default.as_ref())
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    codes::STEP_015,
                    format!("step '{}' requires input '{name}'", file.name),
                )
            })?;
        if !type_matches(&spec.kind, &value) {
            return Err(Error::new(
                codes::STEP_016,
                format!(
                    "input '{name}' of step '{}' must be {}, got {}",
                    file.name,
                    spec.kind,
                    kind(&value)
                ),
            ));
        }
        scope.insert(format!("inputs.{name}"), value);
    }
    Ok(scope)
}

fn type_matches(kind: &str, value: &Value) -> bool {
    match kind {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "boolean" => value.is_boolean(),
        "list<string>" => value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string)),
        "list<map>" => value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_object)),
        _ => false,
    }
}

/// Guest paths belong to the executor. A step that spells one out breaks
/// silently when the executor moves it, and it teaches the wrong thing to every
/// recipe author who copies the step. Reject it for built-in, local and
/// SRPM-embedded steps alike.
fn validate_guest_paths(name: &str, file: &StepFile) -> Result<()> {
    const GUEST_PATHS: [&str; 4] = [
        rpmx_base::guest::BUILD,
        rpmx_base::guest::STAGE,
        rpmx_base::guest::PREFIX,
        rpmx_base::guest::LIBDIR,
    ];

    let run = &file.run;
    let mut fields: Vec<(String, &str)> = Vec::new();
    if let Some(cwd) = run.cwd.as_deref() {
        fields.push(("cwd".into(), cwd));
    }
    if let Some(runs) = run.runs.as_deref() {
        fields.push(("runs".into(), runs));
    }
    for (index, arg) in run.argv.iter().flatten().enumerate() {
        fields.push((format!("argv[{index}]"), arg));
    }
    for (key, value) in run.env.iter().flatten() {
        fields.push((format!("env.{key}"), value));
    }
    for (input, spec) in &file.inputs {
        if let Some(Value::String(value)) = &spec.default {
            fields.push((format!("inputs.{input}.default"), value));
        }
    }

    for (field, text) in fields {
        for path in GUEST_PATHS {
            if text.contains(path) {
                return Err(Error::new(
                    codes::STEP_019,
                    format!(
                        "step '{name}' hardcodes guest path '{path}' in '{field}'; \
                         use the sandbox variable ($STAGE, $PREFIX, $LIBDIR) in 'runs', \
                         or the ${{...}} placeholder in 'cwd', 'argv' and 'env'"
                    ),
                )
                .at(field));
            }
        }
    }
    Ok(())
}

fn render_run(
    run: &Run,
    scope: &BTreeMap<String, Value>,
    name: &str,
) -> Result<(String, Vec<String>)> {
    match (&run.argv, &run.runs) {
        (Some(argv), None) => {
            if argv.is_empty() {
                return Err(Error::new(
                    codes::STEP_017,
                    format!("step '{name}' has an empty argv"),
                ));
            }
            let mut expanded = Vec::with_capacity(argv.len());
            for arg in argv {
                expanded.push(template::expand_inputs(arg, scope)?);
            }
            let command = expanded.remove(0);
            Ok((command, expanded))
        }
        (None, Some(runs)) => Ok((
            "sh".to_string(),
            vec!["-c".to_string(), template::expand_inputs(runs, scope)?],
        )),
        _ => Err(Error::new(
            codes::STEP_017,
            format!("step '{name}' must have exactly one of 'argv' or 'runs'"),
        )),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_steps_do_not_hardcode_guest_paths_in_cwd() {
        for (name, source) in BUILTINS {
            let file: StepFile = serde_yaml_ng::from_str(source)
                .unwrap_or_else(|error| panic!("built-in step '{name}' is invalid: {error}"));
            if let Some(cwd) = &file.run.cwd {
                assert!(
                    cwd.starts_with("${"),
                    "built-in step '{name}' hardcodes cwd '{cwd}'; use a ${{...}} placeholder so a recipe author never needs to know a guest path"
                );
            }
        }
    }

    #[test]
    fn autotools_configure_supports_an_out_of_tree_build() {
        let with = BTreeMap::from([
            ("dir".to_string(), serde_json::json!("build")),
            ("configure".to_string(), serde_json::json!("../configure")),
            (
                "args".to_string(),
                serde_json::json!(["--enable-kernel=4.4"]),
            ),
        ]);
        let step = resolve("autotools/configure", None, Some(&with), false).expect("step");
        let script = &step.args[1];
        assert!(script.contains("cd \"build\""), "{script}");
        assert!(script.contains("\"../configure\""), "{script}");
        assert!(script.contains("--enable-kernel=4.4"), "{script}");
    }

    #[test]
    fn license_copy_names_every_output() {
        let with = BTreeMap::from([
            ("files".to_string(), serde_json::json!(["COPYING"])),
            (
                "outputs".to_string(),
                serde_json::json!(["demo", "demo-dev"]),
            ),
        ]);
        let step = resolve("license/copy", None, Some(&with), false).expect("step");
        let script = &step.args[1];
        assert!(script.contains("for output in demo demo-dev"), "{script}");
        assert!(
            script.contains("$STAGE${prefix}/share/licenses/$output"),
            "{script}"
        );
    }

    #[test]
    fn make_build_uses_the_sandbox_job_count() {
        let step = resolve("make/build", None, None, false).expect("step");
        let script = &step.args[1];
        assert!(script.contains("-j\"$JOBS\""), "{script}");
        assert!(!script.contains("-j1"), "{script}");
    }

    #[test]
    fn builtin_steps_do_not_hardcode_guest_paths_in_their_body() {
        let guest_paths = [
            rpmx_base::guest::BUILD,
            rpmx_base::guest::STAGE,
            rpmx_base::guest::PREFIX,
            rpmx_base::guest::LIBDIR,
        ];
        for (name, source) in BUILTINS {
            for path in guest_paths {
                assert!(
                    !source.contains(path),
                    "built-in step '{name}' hardcodes guest path '{path}'; use the sandbox variable or the ${{...}} placeholder"
                );
            }
        }
    }
}
