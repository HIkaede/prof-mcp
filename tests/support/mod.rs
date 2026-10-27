#![allow(dead_code)]

use std::{io::Cursor, path::PathBuf};

use prof_mcp::profile::{BuildLimits, Profile, ProfileBuilder};

pub fn profile(input: &str) -> Profile {
    ProfileBuilder::new(BuildLimits::default())
        .from_reader(
            Cursor::new(input.as_bytes()),
            PathBuf::from("/synthetic/profile.folded"),
        )
        .expect("synthetic profile must parse")
}

pub fn frame(profile: &Profile, name: &str) -> u32 {
    profile.frame_id(name).expect("fixture frame")
}

pub const DIAMOND: &str = "root;A;X 20\nroot;B;X 30\n";
pub const SHARED_CALLEE: &str = "root;A;X;leaf1 20\nroot;B;X;leaf2 30\n";
pub const RECURSION: &str = "root;foo;foo;bar 10\n";

pub fn json(value: impl serde::Serialize) -> serde_json::Value {
    normalize(serde_json::to_value(value).expect("query output serializes"))
}

/// Rebuild columnar tables (`{columns, rows}`) into one object per row so
/// assertions can read a row by field name. Tests that need to check the wire
/// format itself serialize directly instead of going through [`json`].
pub fn normalize(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let columns = map
                .get("columns")
                .and_then(|value| value.as_array())
                .cloned();
            let rows = map.get("rows").and_then(|value| value.as_array()).cloned();
            if let (Some(columns), Some(rows)) = (columns, rows) {
                return rows
                    .into_iter()
                    .map(|row| {
                        let cells = row.as_array().cloned().unwrap_or_default();
                        columns
                            .iter()
                            .filter_map(|column| column.as_str())
                            .map(str::to_owned)
                            .zip(cells)
                            .collect::<serde_json::Map<_, _>>()
                            .into()
                    })
                    .collect::<Vec<serde_json::Value>>()
                    .into();
            }
            map.into_iter()
                .map(|(key, value)| (key, normalize(value)))
                .collect::<serde_json::Map<_, _>>()
                .into()
        }
        serde_json::Value::Array(items) => {
            items.into_iter().map(normalize).collect::<Vec<_>>().into()
        }
        other => other,
    }
}
