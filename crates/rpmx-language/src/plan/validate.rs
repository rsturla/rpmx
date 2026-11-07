use std::collections::BTreeSet;

use crate::model::{Output, Recipe, Step, Tool};
use rpmx_base::error::{codes, Code, Error, Result};

use super::SCHEMA_VERSION;

pub(crate) fn validate_sha512(
    name: &str,
    digest: &str,
    path: String,
    algorithm_code: Code,
    malformed_code: Code,
    noun: &str,
) -> Result<()> {
    let (algorithm, hex) = digest.split_once(':').unwrap_or(("unprefixed", digest));
    let expected = match algorithm {
        "sha512" => 128,
        "sha256" => 64,
        _ => {
            return Err(Error::new(
                algorithm_code,
                format!(
                    "{noun} '{name}' uses digest algorithm '{algorithm}'; rpmx requires sha512 or sha256"
                ),
            )
            .at(path))
        }
    };
    if hex.len() != expected || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::new(
            malformed_code,
            format!("{noun} '{name}' has an invalid {algorithm} digest"),
        )
        .at(path));
    }
    Ok(())
}

pub(crate) fn validate_tool(tool: &Tool, index: usize) -> Result<()> {
    let path = |field: &str| format!("source-assembly.tools[{index}].{field}");
    if tool.name.trim().is_empty() || tool.version.trim().is_empty() {
        return Err(
            Error::new(codes::TOOL_004, "tool name and version must not be empty").at(path("name")),
        );
    }
    validate_sha512(
        &tool.name,
        &tool.digest,
        path("digest"),
        codes::TOOL_001,
        codes::TOOL_002,
        "tool",
    )?;
    if tool.locators.is_empty() {
        return Err(Error::new(
            codes::TOOL_005,
            format!("tool '{}' has no locators", tool.name),
        )
        .at(path("locators")));
    }
    for (locator_index, locator) in tool.locators.iter().enumerate() {
        if locator.kind.trim().is_empty() {
            return Err(
                Error::new(codes::TOOL_006, "locator kind must not be empty").at(format!(
                    "source-assembly.tools[{index}].locators[{locator_index}]"
                )),
            );
        }
    }
    Ok(())
}

pub(crate) fn validate_rpm_component(field: &str, value: &str) -> Result<()> {
    if value.contains('/')
        || value.contains('\\')
        || value.contains("..")
        || value.chars().any(char::is_whitespace)
        || value == "."
    {
        return Err(Error::new(
            codes::PLAN_011,
            format!("{field} '{value}' is not a valid RPM name/version/release"),
        )
        .at(field));
    }
    Ok(())
}

pub(crate) fn validate_schema(recipe: &Recipe) -> Result<()> {
    if recipe.schema != SCHEMA_VERSION {
        return Err(Error::new(
            codes::SCHEMA_001,
            format!(
                "unsupported schema {}; this build of rpmx supports schema {}",
                recipe.schema, SCHEMA_VERSION
            ),
        )
        .at("schema"));
    }
    Ok(())
}

pub(crate) fn validate_cpe(recipe: &Recipe) -> Result<()> {
    let Some(cpe) = &recipe.package.cpe else {
        return Ok(());
    };
    if cpe.part != "a" && cpe.part != "o" {
        return Err(Error::new(
            codes::SCHEMA_002,
            format!(
                "CPE part '{}' must be 'a' (application) or 'o' (operating system)",
                cpe.part
            ),
        )
        .at("package.cpe.part"));
    }
    for (field, value) in [
        ("vendor", Some(cpe.vendor.as_str())),
        ("product", Some(cpe.product.as_str())),
        ("edition", cpe.edition.as_deref()),
        ("language", cpe.language.as_deref()),
        ("sw-edition", cpe.sw_edition.as_deref()),
        ("target-sw", cpe.target_sw.as_deref()),
        ("target-hw", cpe.target_hw.as_deref()),
        ("other", cpe.other.as_deref()),
    ] {
        if let Some(value) = value {
            if value.is_empty()
                || !value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
                })
            {
                return Err(Error::new(
                    codes::SCHEMA_002,
                    format!(
                        "CPE {field} '{value}' must use only ASCII letters, digits, '_', '-' and '.'"
                    ),
                )
                .at(format!("package.cpe.{field}")));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_sources(recipe: &Recipe) -> Result<()> {
    for (index, source) in recipe.sources.iter().enumerate() {
        let path = format!("sources[{index}].digest");
        validate_sha512(
            &source.name,
            &source.digest,
            path,
            codes::SOURCE_001,
            codes::SOURCE_002,
            "source",
        )?;
        if source.name.is_empty()
            || source.name == "."
            || source.name == ".."
            || source.name.contains('/')
            || source.name.contains('\\')
        {
            return Err(Error::new(
                codes::SOURCE_002,
                format!(
                    "source name '{}' must be a single relative path component",
                    source.name
                ),
            )
            .at(format!("sources[{index}].name")));
        }
        if let Some(extract) = &source.extract {
            if let Some(destination) = &extract.destination {
                if destination.starts_with('/')
                    || destination.split(['/', '\\']).any(|part| part == "..")
                {
                    return Err(Error::new(
                        codes::SOURCE_002,
                        format!(
                            "source '{}' extract.destination '{}' must stay under the workspace",
                            source.name, destination
                        ),
                    )
                    .at(format!("sources[{index}].extract.destination")));
                }
            }
            if let Some(format) = &extract.format {
                if !matches!(
                    format.as_str(),
                    "auto" | "tar" | "tar.gz" | "tgz" | "tar.xz" | "tar.zst" | "zstd" | "none"
                ) {
                    return Err(Error::new(
                        codes::SOURCE_002,
                        format!(
                            "source '{}' has unsupported extract.format '{}'",
                            source.name, format
                        ),
                    )
                    .at(format!("sources[{index}].extract.format")));
                }
            }
        }
        for (locator_index, locator) in source.locators.iter().enumerate() {
            if locator.kind != "git" {
                continue;
            }
            let base = format!("sources[{index}].locators[{locator_index}]");
            if locator.url.as_deref().is_none_or(str::is_empty) {
                return Err(Error::new(
                    codes::SOURCE_002,
                    format!("git locator for '{}' has no url", source.name),
                )
                .at(base));
            }
            let Some(commit) = locator.commit.as_deref() else {
                return Err(Error::new(
                    codes::SOURCE_002,
                    format!(
                        "git locator for '{}' has no commit; pin the full commit hash",
                        source.name
                    ),
                )
                .at(base));
            };
            if !(commit.len() == 40 || commit.len() == 64)
                || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(Error::new(
                    codes::SOURCE_002,
                    format!(
                        "git commit for '{}' must be a full 40- or 64-character hex hash",
                        source.name
                    ),
                )
                .at(base));
            }
            if source.name.is_empty()
                || source.name.contains('/')
                || source.name == "."
                || source.name == ".."
            {
                return Err(Error::new(
                    codes::SOURCE_002,
                    format!(
                        "git source name '{}' cannot be used as a directory prefix",
                        source.name
                    ),
                )
                .at(format!("sources[{index}].name")));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_steps(recipe: &Recipe) -> Result<()> {
    for (index, step) in recipe.build.pipeline.iter().enumerate() {
        validate_step(step, format!("build.pipeline[{index}]"))?;
    }
    for (index, test) in recipe.tests.iter().enumerate() {
        if let Some(pipeline) = &test.pipeline {
            for (step_index, step) in pipeline.iter().enumerate() {
                validate_step(step, format!("tests[{index}].pipeline[{step_index}]"))?;
            }
        }
    }
    for (output_index, output) in recipe.outputs.iter().enumerate() {
        for (test_index, test) in output.tests.iter().enumerate() {
            if let Some(pipeline) = &test.pipeline {
                for (step_index, step) in pipeline.iter().enumerate() {
                    validate_step(
                        step,
                        format!(
                            "outputs[{output_index}].tests[{test_index}].pipeline[{step_index}]"
                        ),
                    )?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_step(step: &Step, path: String) -> Result<()> {
    let forms = [
        step.uses.is_some(),
        step.argv.is_some(),
        step.runs.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();

    if forms != 1 {
        return Err(Error::new(
            codes::STEP_001,
            format!(
                "step '{}' must have exactly one of 'uses', 'argv' or 'runs'",
                step.label()
            ),
        )
        .at(path));
    }
    if step.argv.as_ref().is_some_and(Vec::is_empty) {
        return Err(Error::new(codes::STEP_002, "argv must not be empty").at(path));
    }
    if step
        .runs
        .as_ref()
        .is_some_and(|runs| runs.trim().is_empty())
    {
        return Err(Error::new(codes::STEP_003, "runs must not be empty").at(path));
    }
    if let Some((guest_path, field)) = hardcoded_guest_path(step) {
        return Err(Error::new(
            codes::STEP_019,
            format!(
                "step '{}' hardcodes guest path '{guest_path}' in '{field}'; \
                 use the ${{...}} placeholder so the executor supplies it",
                step.label()
            ),
        )
        .at(format!("{path}.{field}")));
    }
    Ok(())
}

fn hardcoded_guest_path(step: &Step) -> Option<(&'static str, String)> {
    let mut fields: Vec<(String, &str)> = Vec::new();
    if let Some(cwd) = step.cwd.as_deref() {
        fields.push(("cwd".into(), cwd));
    }
    if let Some(runs) = step.runs.as_deref() {
        fields.push(("runs".into(), runs));
    }
    for (index, arg) in step.argv.iter().flatten().enumerate() {
        fields.push((format!("argv[{index}]"), arg));
    }
    for (key, value) in step.env.iter().flatten() {
        fields.push((format!("env.{key}"), value));
    }
    for (field, text) in fields {
        for guest_path in [rpmx_base::guest::BUILD, rpmx_base::guest::STAGE] {
            if text.contains(guest_path) {
                return Some((guest_path, field));
            }
        }
    }
    None
}

pub(crate) fn validate_outputs(recipe: &Recipe) -> Result<()> {
    if recipe.outputs.is_empty() {
        return Err(Error::new(codes::OUTPUT_001, "at least one output is required").at("outputs"));
    }
    for (index, output) in recipe.outputs.iter().enumerate() {
        if output.name.trim().is_empty() {
            return Err(Error::new(codes::OUTPUT_002, "output name is empty")
                .at(format!("outputs[{index}].name")));
        }
        if output.files.include.is_empty() && !output.metapackage {
            return Err(Error::new(
                codes::OUTPUT_003,
                format!(
                    "output '{}' has no file selectors; set metapackage: true if that is intended",
                    output.name
                ),
            )
            .at(format!("outputs[{index}].files.include")));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_file_metadata(output: &Output) -> Result<()> {
    let mut directories = BTreeSet::new();
    for directory in &output.files.directories {
        validate_metadata_path(&directory.path, "directory")?;
        if !directories.insert(directory.path.as_str()) {
            return Err(Error::new(
                codes::OUTPUT_017,
                format!(
                    "output '{}' declares directory '{}' twice",
                    output.name, directory.path
                ),
            ));
        }
        if let Some(mode) = &directory.mode {
            validate_octal_mode(mode)?;
        }
        validate_identity(directory.user.as_deref(), "user", &directory.path)?;
        validate_identity(directory.group.as_deref(), "group", &directory.path)?;
    }

    let mut attributes = BTreeSet::new();
    for attribute in &output.files.attributes {
        validate_metadata_path(&attribute.path, "attribute")?;
        if directories.contains(attribute.path.as_str()) {
            return Err(Error::new(
                codes::OUTPUT_017,
                format!(
                    "output '{}' declares '{}' as both a directory and a file attribute",
                    output.name, attribute.path
                ),
            ));
        }
        if !attributes.insert(attribute.path.as_str()) {
            return Err(Error::new(
                codes::OUTPUT_017,
                format!(
                    "output '{}' declares attributes for '{}' twice",
                    output.name, attribute.path
                ),
            ));
        }
        if let Some(mode) = &attribute.mode {
            validate_octal_mode(mode)?;
        }
        validate_identity(attribute.user.as_deref(), "user", &attribute.path)?;
        validate_identity(attribute.group.as_deref(), "group", &attribute.path)?;
        if let Some(caps) = &attribute.caps {
            if caps.trim().is_empty() || caps.chars().any(char::is_whitespace) {
                return Err(Error::new(
                    codes::OUTPUT_016,
                    format!(
                        "capabilities for '{}' must be a non-empty RPM capability string such as cap_net_raw=ep",
                        attribute.path
                    ),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_metadata_path(path: &str, kind: &str) -> Result<()> {
    if !path.starts_with('/')
        || path == "/"
        || path.ends_with('/')
        || path.split('/').any(|component| component == "..")
    {
        return Err(Error::new(
            codes::OUTPUT_015,
            format!("{kind} path '{path}' must be absolute, normalised and not the root"),
        ));
    }
    Ok(())
}

pub(crate) fn validate_octal_mode(mode: &str) -> Result<()> {
    let valid =
        (3..=4).contains(&mode.len()) && mode.bytes().all(|byte| (b'0'..=b'7').contains(&byte));
    if !valid {
        return Err(Error::new(
            codes::OUTPUT_016,
            format!("mode '{mode}' must be a 3- or 4-digit octal string such as \"0755\""),
        ));
    }
    Ok(())
}

pub(crate) fn validate_identity(value: Option<&str>, field: &str, path: &str) -> Result<()> {
    if let Some(value) = value {
        if value.trim().is_empty() || value.contains(':') || value.contains('\n') {
            return Err(Error::new(
                codes::OUTPUT_016,
                format!("{field} for '{path}' is not a valid name"),
            ));
        }
    }
    Ok(())
}
