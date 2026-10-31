pub mod arch;
pub mod binary_package;
pub mod cas;
pub mod elf;
pub mod error;
pub mod fetch;
pub mod matrix;
pub mod model;
pub mod package;
pub mod pkgmanifest;
pub mod plan;
pub mod profile;
pub mod root;
pub mod sandbox;
pub mod sbom;
pub mod source;
pub mod source_package;
pub mod srpm;
pub mod steps;
pub mod template;
pub mod tree;

use std::path::Path;

use crate::error::codes;
pub use arch::Arch;
pub use error::{Error, Result};
pub use model::Recipe;
pub use plan::{
    normalize_dist_tag, plan, plan_with_options, Plan, PlanOptions, PlannedCell, PlannedOutput,
    PlannedStep, PlannedTest,
};

pub fn parse_recipe(text: &str) -> Result<Recipe> {
    reject_yaml_constructs(text)?;
    serde_yaml_ng::from_str(text).map_err(|error| {
        let location = error.location();
        let mapped = Error::new(codes::YAML_001, error.to_string());
        match location {
            Some(location) => mapped.with_location(Some(location.line()), Some(location.column())),
            None => mapped,
        }
    })
}

pub(crate) fn reject_yaml_constructs(text: &str) -> Result<()> {
    use saphyr_parser::{Event, Parser};

    let mut parser = Parser::new_from_str(text);
    while let Some(event) = parser.next_event() {
        let (event, _span) =
            event.map_err(|error| Error::new(codes::YAML_002, format!("invalid YAML: {error}")))?;
        match event {
            Event::Alias(_) => {
                return Err(Error::new(
                    codes::YAML_003,
                    "YAML aliases are not allowed; write the value in full",
                ));
            }
            Event::Scalar(_, _, anchor, tag)
            | Event::SequenceStart(anchor, tag)
            | Event::MappingStart(anchor, tag) => {
                if anchor != 0 {
                    return Err(Error::new(
                        codes::YAML_003,
                        "YAML anchors are not allowed; write the value in full",
                    ));
                }
                if tag.is_some_and(|tag| !tag.is_yaml_core_schema()) {
                    return Err(Error::new(
                        codes::YAML_004,
                        "custom YAML tags are not allowed",
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn load_recipe(path: impl AsRef<Path>) -> Result<Recipe> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::new(
            codes::IO_001,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    parse_recipe(&text)
}
