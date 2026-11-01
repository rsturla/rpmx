use std::collections::BTreeMap;

use serde_json::Value;

use rpmx_base::error::{codes, Error, Result};

pub fn expand_vars(
    raw: &BTreeMap<String, Value>,
    base: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, String>> {
    let mut cache = BTreeMap::new();
    for name in raw.keys() {
        expand_var(name, raw, base, &mut cache, &mut Vec::new())?;
    }
    Ok(cache)
}

fn expand_var(
    name: &str,
    raw: &BTreeMap<String, Value>,
    base: &BTreeMap<String, Value>,
    cache: &mut BTreeMap<String, String>,
    stack: &mut Vec<String>,
) -> Result<String> {
    if let Some(value) = cache.get(name) {
        return Ok(value.clone());
    }
    if stack.iter().any(|item| item == name) {
        let mut cycle = stack.clone();
        cycle.push(name.to_string());
        return Err(Error::new(
            codes::TEMPLATE_001,
            format!("variable cycle: {}", cycle.join(" -> ")),
        )
        .at(format!("vars.{name}")));
    }

    let value = raw.get(name).ok_or_else(|| {
        Error::new(codes::TEMPLATE_002, format!("unknown variable '{name}'"))
            .at(format!("vars.{name}"))
    })?;
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(boolean) => boolean.to_string(),
        other => {
            return Err(Error::new(
                codes::TEMPLATE_003,
                format!("vars.{name} must be a scalar, got {}", kind(other)),
            )
            .at(format!("vars.{name}")));
        }
    };

    stack.push(name.to_string());
    let expanded = expand_tokens(&text, &mut |token| {
        if let Some(rest) = token.strip_prefix("vars.") {
            expand_var(rest, raw, base, cache, stack)
        } else {
            lookup(base, token)
        }
    })?;
    stack.pop();

    cache.insert(name.to_string(), expanded.clone());
    Ok(expanded)
}

pub fn expand_string(input: &str, scope: &BTreeMap<String, Value>) -> Result<String> {
    expand_tokens(input, &mut |token| lookup(scope, token))
}

pub fn expand_inputs(input: &str, inputs: &BTreeMap<String, Value>) -> Result<String> {
    expand_tokens_inner(
        input,
        &mut |token| {
            if let Some(name) = token.strip_prefix("inputs.") {
                let value = inputs.get(token).ok_or_else(|| {
                    Error::new(
                        codes::STEP_015,
                        format!("unknown input '${{inputs.{name}}}'"),
                    )
                })?;
                match value {
                    Value::Array(items) => {
                        let mut parts = Vec::with_capacity(items.len());
                        for item in items {
                            parts.push(value_to_string(item, token)?);
                        }
                        Ok(parts.join(" "))
                    }
                    other => value_to_string(other, token),
                }
            } else {
                Ok(format!("${{{token}}}"))
            }
        },
        false,
    )
}

pub fn expand_json(value: &Value, scope: &BTreeMap<String, Value>) -> Result<Value> {
    match value {
        Value::String(text) => Ok(Value::String(expand_string(text, scope)?)),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(expand_json(item, scope)?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, item) in map {
                out.insert(key.clone(), expand_json(item, scope)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

fn expand_tokens(input: &str, resolve: &mut impl FnMut(&str) -> Result<String>) -> Result<String> {
    expand_tokens_inner(input, resolve, true)
}

fn expand_tokens_inner(
    input: &str,
    resolve: &mut impl FnMut(&str) -> Result<String>,
    reject_unresolved: bool,
) -> Result<String> {
    let mut out = String::with_capacity(input.len());
    let mut index = 0;
    const ESCAPED_DOLLAR: char = '\u{0}';

    while index < input.len() {
        let rest = &input[index..];
        if rest.starts_with("$${") {
            out.push(ESCAPED_DOLLAR);
            out.push('{');
            index += 3;
            continue;
        }
        if let Some(body) = rest.strip_prefix("${") {
            let end = body.find('}').ok_or_else(|| {
                Error::new(
                    codes::TEMPLATE_004,
                    format!("unterminated template in '{input}'"),
                )
            })?;
            let token = &body[..end];
            out.push_str(&resolve(token)?);
            index += 2 + end + 1;
            continue;
        }

        let ch = rest.chars().next().expect("non-empty remainder");
        out.push(ch);
        index += ch.len_utf8();
    }

    if reject_unresolved && out.contains("${") {
        return Err(Error::new(
            codes::TEMPLATE_007,
            format!("template in '{input}' produced an unresolved '${{...}}' value"),
        ));
    }

    Ok(out.replace(ESCAPED_DOLLAR, "$"))
}

fn lookup(scope: &BTreeMap<String, Value>, token: &str) -> Result<String> {
    let value = scope.get(token).ok_or_else(|| {
        Error::new(
            codes::TEMPLATE_005,
            format!("unknown variable '${{{token}}}'"),
        )
    })?;
    value_to_string(value, token)
}

fn value_to_string(value: &Value, token: &str) -> Result<String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(boolean) => Ok(boolean.to_string()),
        Value::Null => Ok(String::new()),
        other => Err(Error::new(
            codes::TEMPLATE_006,
            format!("'${{{token}}}' is a {}, not a scalar", kind(other)),
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
