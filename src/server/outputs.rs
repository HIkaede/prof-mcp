use std::sync::Arc;

use schemars::JsonSchema;
use serde_json::{Value, json};

pub(crate) use crate::output::{
    DiffOutput, DirectionOutput, FindOutput, PathsOutput, SummaryOutput, TopOutput, TreeOutput,
};

pub(crate) fn typed_output_schema<T: JsonSchema>() -> Arc<serde_json::Map<String, Value>> {
    let mut schema = serde_json::to_value(schemars::schema_for!(T))
        .expect("static output schema serializes")
        .as_object()
        .expect("static output schema is object")
        .clone();
    let required = schema
        .remove("required")
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let properties = schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .expect("derived output schema has properties");
    properties.insert(
        "schema_version".into(),
        json!({"type":"string","const":"2"}),
    );
    properties.insert("code".into(), json!({"type":"string"}));
    properties.insert("message".into(), json!({"type":"string"}));
    properties.insert("details".into(), json!({}));
    properties.insert("retry_hint".into(), json!({"type":"string"}));
    schema.insert(
        "anyOf".into(),
        json!([
            {"required":required},
            {"required":["code","message","details","retry_hint"]}
        ]),
    );
    schema.into()
}
