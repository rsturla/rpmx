use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha512};

use crate::error::{codes, Error, Result};
use crate::template;

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

pub fn resolve(
    name: &str,
    root: Option<&Path>,
    with: Option<&BTreeMap<String, Value>>,
    allow_builtin_override: bool,
) -> Result<ResolvedStep> {
    let local = local_path(root, name).filter(|path| path.exists());
    let builtin = builtin_source(name);

    let (text, builtin) = match (builtin, local) {
        (Some(_), Some(_)) if !allow_builtin_override => {
            return Err(Error::new(
                codes::STEP_016,
                format!("local step '{name}' would shadow a built-in step"),
            ));
        }
        (_, Some(path)) => {
            let bytes = std::fs::read(&path).map_err(|error| {
                Error::new(
                    codes::STEP_011,
                    format!("cannot read step '{}': {error}", path.display()),
                )
            })?;
            let text = String::from_utf8(bytes).map_err(|error| {
                Error::new(
                    codes::STEP_012,
                    format!("step '{}' is not valid UTF-8: {error}", path.display()),
                )
            })?;
            (text, false)
        }
        (Some(source), None) => (source.to_string(), true),
        (None, None) => {
            validate_name(name)?;
            let root = root.ok_or_else(|| {
                Error::new(
                    codes::STEP_011,
                    format!("unknown step '{name}' and no pipeline directory is configured"),
                )
            })?;
            let path = root.join(format!("{name}.yaml"));
            let bytes = std::fs::read(&path).map_err(|error| {
                Error::new(
                    codes::STEP_011,
                    format!(
                        "unknown step '{name}': cannot read {}: {error}",
                        path.display()
                    ),
                )
            })?;
            let text = String::from_utf8(bytes).map_err(|error| {
                Error::new(
                    codes::STEP_012,
                    format!("step '{}' is not valid UTF-8: {error}", path.display()),
                )
            })?;
            (text, false)
        }
    };

    let digest = format!("sha512:{:x}", Sha512::digest(text.as_bytes()));
    let file: StepFile = serde_yaml_ng::from_str(&text).map_err(|error| {
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

    let mut needs = file.needs.tools.clone();
    needs.extend(file.needs.host);

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
