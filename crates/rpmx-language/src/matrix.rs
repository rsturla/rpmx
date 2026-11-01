use std::collections::BTreeMap;

use serde_json::Value;

use crate::model::Matrix;
use rpmx_base::error::{codes, Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub id: String,
    pub slug: String,
    pub identity: BTreeMap<String, Value>,
    pub fields: BTreeMap<String, BTreeMap<String, Value>>,
}

pub fn expand(matrix: Option<&Matrix>) -> Result<Vec<Cell>> {
    let Some(matrix) = matrix else {
        return Ok(vec![Cell {
            id: String::new(),
            slug: String::new(),
            identity: BTreeMap::new(),
            fields: BTreeMap::new(),
        }]);
    };

    if matrix.axes.is_empty() {
        return Err(Error::new(
            codes::MATRIX_001,
            "matrix.axes is empty; remove the matrix block or declare an axis",
        ));
    }

    for (index, exclude) in matrix.exclude.iter().enumerate() {
        for (axis, wanted) in exclude {
            if !matrix.axes.contains_key(axis) {
                return Err(Error::new(
                    codes::MATRIX_010,
                    format!("matrix.exclude[{index}] references unknown axis '{axis}'"),
                )
                .at(format!("matrix.exclude[{index}].{axis}")));
            }
            if !is_scalar(wanted) {
                return Err(Error::new(
                    codes::MATRIX_011,
                    format!("matrix.exclude[{index}].{axis} must be a scalar"),
                )
                .at(format!("matrix.exclude[{index}].{axis}")));
            }
        }
    }

    type Combination = (
        BTreeMap<String, Value>,
        BTreeMap<String, BTreeMap<String, Value>>,
    );

    let mut combinations: Vec<Combination> = vec![(BTreeMap::new(), BTreeMap::new())];

    let mut normalized = Vec::with_capacity(matrix.axes.len());
    for (axis, values) in &matrix.axes {
        if values.is_empty() {
            return Err(
                Error::new(codes::MATRIX_002, format!("axis '{axis}' has no values"))
                    .at(format!("matrix.axes.{axis}")),
            );
        }
        let values = values
            .iter()
            .map(|value| split_value(axis, value))
            .collect::<Result<Vec<_>>>()?;
        normalized.push((axis, values));
    }

    for (axis, values) in normalized {
        let mut next = Vec::new();
        for (identity, fields) in &combinations {
            for (identity_value, value_fields) in &values {
                let mut next_identity = identity.clone();
                next_identity.insert(axis.clone(), identity_value.clone());
                let mut next_fields = fields.clone();
                if !value_fields.is_empty() {
                    next_fields.insert(axis.clone(), value_fields.clone());
                }
                next.push((next_identity, next_fields));
            }
        }
        combinations = next;
    }

    let limit = matrix.max_cells.unwrap_or(32);
    if combinations.len() > limit {
        return Err(Error::new(
            codes::MATRIX_003,
            format!(
                "matrix expands to {} cells, above max-cells={limit}",
                combinations.len()
            ),
        )
        .at("matrix.max-cells"));
    }

    let mut cells = Vec::with_capacity(combinations.len());
    for (identity, fields) in combinations {
        if matches_exclude(&identity, &matrix.exclude) {
            continue;
        }

        let id = identity
            .iter()
            .map(|(axis, value)| format!("{axis}={}", scalar(value)))
            .collect::<Vec<_>>()
            .join(",");

        let slug = identity
            .iter()
            .map(|(axis, value)| {
                fields
                    .get(axis)
                    .and_then(|f| f.get("slug"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{axis}-{}", normalize(&scalar(value))))
            })
            .collect::<Vec<_>>()
            .join("-");

        cells.push(Cell {
            id,
            slug,
            identity,
            fields,
        });
    }

    if cells.is_empty() {
        return Err(
            Error::new(codes::MATRIX_004, "matrix.exclude removes every cell").at("matrix.exclude"),
        );
    }

    let mut ids = BTreeMap::new();
    let mut slugs = BTreeMap::new();
    for cell in &cells {
        if let Some(previous) = ids.insert(cell.id.clone(), cell.slug.clone()) {
            return Err(Error::new(
                codes::MATRIX_005,
                format!(
                    "cell id '{}' is produced twice (slugs '{previous}' and '{}')",
                    cell.id, cell.slug
                ),
            ));
        }
        if !cell.slug.is_empty() {
            if let Some(previous) = slugs.insert(cell.slug.clone(), cell.id.clone()) {
                return Err(Error::new(
                    codes::MATRIX_006,
                    format!(
                        "cell slug '{}' is produced by both '{}' and '{}'",
                        cell.slug, previous, cell.id
                    ),
                ));
            }
        }
    }

    Ok(cells)
}

fn split_value(axis: &str, value: &Value) -> Result<(Value, BTreeMap<String, Value>)> {
    match value {
        Value::Object(map) => {
            let identity = map.get("value").cloned().ok_or_else(|| {
                Error::new(
                    codes::MATRIX_007,
                    format!("map value on axis '{axis}' must define a scalar 'value'"),
                )
                .at(format!("matrix.axes.{axis}"))
            })?;
            if !is_scalar(&identity) {
                return Err(Error::new(
                    codes::MATRIX_008,
                    format!("'value' on axis '{axis}' must be a scalar"),
                )
                .at(format!("matrix.axes.{axis}.value")));
            }
            Ok((identity, map.clone().into_iter().collect()))
        }
        other if is_scalar(other) => Ok((other.clone(), BTreeMap::new())),
        _ => Err(Error::new(
            codes::MATRIX_009,
            format!("axis '{axis}' values must be scalars or maps with 'value'"),
        )
        .at(format!("matrix.axes.{axis}"))),
    }
}

fn matches_exclude(
    identity: &BTreeMap<String, Value>,
    excludes: &[BTreeMap<String, Value>],
) -> bool {
    for exclude in excludes {
        let mut matches = true;
        for (axis, wanted) in exclude {
            let Some(actual) = identity.get(axis) else {
                matches = false;
                break;
            };
            if !same_scalar(actual, wanted) {
                matches = false;
                break;
            }
        }
        if matches {
            return true;
        }
    }
    false
}

fn same_scalar(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::String(a), Value::Number(b)) => a == &b.to_string(),
        (Value::Number(a), Value::String(b)) => &a.to_string() == b,
        _ => false,
    }
}

fn is_scalar(value: &Value) -> bool {
    matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_))
}

pub fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::Array(_) => "[list]".to_string(),
        Value::Object(_) => "[map]".to_string(),
    }
}

fn normalize(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}
