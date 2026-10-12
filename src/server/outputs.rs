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
    let properties = schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .expect("derived output schema has properties");
    properties.insert(
        "schema_version".into(),
        json!({"type":"string","const":"2"}),
    );
    schema.into()
}
