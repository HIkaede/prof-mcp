use crate::error::ApiError;

pub fn parse_line(
    line_no: usize,
    bytes: &[u8],
    max_depth: usize,
) -> Result<Option<(Vec<&str>, u64)>, ApiError> {
    let raw = std::str::from_utf8(bytes)
        .map_err(|_| invalid_line(line_no, bytes, "contains non-UTF-8 bytes"))?;
    let line = raw
        .trim_end_matches(['\r', '\n'])
        .trim_end_matches(|c: char| c.is_ascii_whitespace());
    if line.is_empty() {
        return Ok(None);
    }
    let weight_start = line
        .rfind(|c: char| c.is_ascii_whitespace())
        .map(|index| index + 1)
        .ok_or_else(|| invalid_line(line_no, bytes, "has no numeric weight"))?;
    let delimiter_start = line[..weight_start - 1]
        .rfind(|c: char| !c.is_ascii_whitespace())
        .map_or(0, |index| index + 1);
    let stack = &line[..delimiter_start];
    let weight_text = &line[weight_start..];
    if stack.is_empty() {
        return Err(invalid_line(line_no, bytes, "has an empty stack"));
    }
    let weight = weight_text.parse::<u64>().map_err(|error| {
        let code = match error.kind() {
            std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow => {
                "weight_overflow"
            }
            _ => "invalid_weight",
        };
        ApiError::new(
            code,
            format!("line {line_no} has invalid weight"),
            serde_json::json!({"line": line_no, "preview": preview(bytes)}),
            "Regenerate the file with stackcollapse-perf.pl.",
        )
    })?;
    if weight == 0 {
        return Err(ApiError::new(
            "invalid_weight",
            format!("line {line_no} has zero weight"),
            serde_json::json!({"line": line_no, "preview": preview(bytes)}),
            "Folded stack weights must be positive.",
        ));
    }
    let frames: Vec<_> = stack.split(';').collect();
    if frames.iter().any(|frame| frame.is_empty()) {
        return Err(invalid_line(line_no, bytes, "has an empty frame"));
    }
    if frames.len() > max_depth {
        return Err(ApiError::new(
            "stack_too_deep",
            format!("line {line_no} exceeds maximum depth {max_depth}"),
            serde_json::json!({"line": line_no, "preview": preview(bytes), "max_depth": max_depth}),
            "Regenerate with a shallower call graph or raise the server limit.",
        ));
    }
    Ok(Some((frames, weight)))
}

pub fn preview(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .take(200)
        .map(|c| if c.is_control() { '�' } else { c })
        .collect()
}

fn invalid_line(line: usize, bytes: &[u8], reason: &str) -> ApiError {
    ApiError::new(
        "invalid_folded_line",
        format!("line {line} {reason}"),
        serde_json::json!({"line": line, "preview": preview(bytes)}),
        "Regenerate the file with stackcollapse-perf.pl.",
    )
}
