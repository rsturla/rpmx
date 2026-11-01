use serde::de::DeserializeOwned;

use crate::error::{codes, Code, Error, Result};

/// Decode a document with the project-wide strict YAML policy.
pub fn parse_strict<T: DeserializeOwned>(text: &str, code: Code) -> Result<T> {
    reject_constructs(text)?;
    serde_yaml_ng::from_str(text).map_err(|error| {
        let mapped = Error::new(code, error.to_string());
        match error.location() {
            Some(location) => mapped.with_location(Some(location.line()), Some(location.column())),
            None => mapped,
        }
    })
}

pub fn reject_constructs(text: &str) -> Result<()> {
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
