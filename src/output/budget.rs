use std::io::{self, Write};

use serde::Serialize;
use serde_json::json;

use crate::error::ApiError;

pub(crate) const MAX_RESPONSE_BYTES: usize = 64 * 1024;

pub(crate) fn response_too_large() -> ApiError {
    ApiError::new(
        "query_too_large",
        "Tool response exceeds its byte limit",
        json!({"resource":"response_bytes", "limit_bytes":MAX_RESPONSE_BYTES}),
        "Reduce limit, max_nodes, max_depth, max_total_frames, or the path window; narrow symbol filters. If one identity is too large, use a profile with shorter symbols.",
    )
}

pub(crate) struct ResponseBudget(pub(crate) usize);

impl ResponseBudget {
    pub(crate) fn new() -> Self {
        Self(MAX_RESPONSE_BYTES)
    }

    pub(crate) fn charge(&mut self, bytes: usize) -> Result<(), ApiError> {
        self.0 = self.0.checked_sub(bytes).ok_or_else(response_too_large)?;
        Ok(())
    }

    pub(crate) fn json(&mut self, value: &impl Serialize) -> Result<(), ApiError> {
        serde_json::to_writer(&mut *self, value).map_err(|_| response_too_large())
    }

    pub(crate) fn row_name(&mut self, name: &str) -> Result<(), ApiError> {
        // Every row has at least 64 encoded bytes of fields besides its name.
        self.charge(64)?;
        self.json(&name)
    }
}

impl Write for ResponseBudget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| io::Error::other("response byte limit exceeded"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_utf8_and_json_escaping_before_allocating() {
        for text in ["abc", "多🙂", "\\\"\n\u{0000}"] {
            let bytes = serde_json::to_vec(text).unwrap().len();
            let mut budget = ResponseBudget(bytes);
            budget.json(&text).unwrap();
            assert_eq!(budget.0, 0);
            assert_eq!(budget.json(&0).unwrap_err().code, "query_too_large");
            assert!(ResponseBudget(bytes - 1).json(&text).is_err());
        }
    }
}
