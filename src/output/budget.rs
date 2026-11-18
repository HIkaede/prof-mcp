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

    pub(crate) fn request_remaining(&self) -> usize {
        self.0
    }

    /// Undo charges made since `request_remaining` was read, so optional payload
    /// can be dropped instead of failing the whole response.
    pub(crate) fn restore(&mut self, remaining: usize) {
        self.0 = remaining;
    }

    pub(crate) fn charge(&mut self, bytes: usize) -> Result<(), ApiError> {
        self.0 = self.0.checked_sub(bytes).ok_or_else(response_too_large)?;
        Ok(())
    }

    pub(crate) fn json(&mut self, value: &impl Serialize) -> Result<(), ApiError> {
        serde_json::to_writer(&mut *self, value).map_err(|_| response_too_large())
    }

    /// Charge JSON serialized inside a JSON string, excluding the outer quotes.
    pub(crate) fn embedded_json(&mut self, value: &impl Serialize) -> Result<(), ApiError> {
        serde_json::to_writer(EmbeddedJson(self), value).map_err(|_| response_too_large())
    }

    pub(crate) fn row_name(&mut self, name: &str) -> Result<(), ApiError> {
        // Every row has at least 64 encoded bytes of fields besides its name.
        self.charge(64)?;
        self.json(&name)
    }

    fn write_bytes(&mut self, count: usize) -> io::Result<()> {
        self.0 = self
            .0
            .checked_sub(count)
            .ok_or_else(|| io::Error::other("response byte limit exceeded"))?;
        Ok(())
    }
}

impl Write for ResponseBudget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_bytes(bytes.len())?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct EmbeddedJson<'a>(&'a mut ResponseBudget);

impl Write for EmbeddedJson<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // The inner serializer has already escaped control characters.
        let escaped = bytes
            .iter()
            .filter(|&&byte| matches!(byte, b'"' | b'\\'))
            .count();
        self.0.write_bytes(bytes.len() + escaped)?;
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

            let inner = serde_json::to_string(text).unwrap();
            let embedded = serde_json::to_vec(&inner).unwrap().len() - 2;
            let mut budget = ResponseBudget(embedded);
            budget.embedded_json(&text).unwrap();
            assert_eq!(budget.0, 0);
            assert!(ResponseBudget(embedded - 1).embedded_json(&text).is_err());
        }
    }
}
