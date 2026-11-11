/// Wrap command-specific fields in the success envelope every JSON command
/// shares.
pub fn success(command: &'static str, value: serde_json::Value) -> serde_json::Value {
    let mut document = match value {
        serde_json::Value::Object(map) => map,
        other => {
            let mut map = serde_json::Map::new();
            map.insert("value".to_string(), other);
            map
        }
    };
    document.insert("schema".to_string(), serde_json::json!(1));
    document.insert("ok".to_string(), serde_json::json!(true));
    document.insert("command".to_string(), serde_json::json!(command));
    serde_json::Value::Object(document)
}
