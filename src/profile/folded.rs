use crate::error::ApiError;

pub fn parse_line(
    line_no: usize,
    bytes: &[u8],
    max_depth: usize,
) -> Result<Option<(Vec<&str>, u64)>, ApiError> {
    let raw = std::str::from_utf8(bytes)
        .map_err(|_| invalid_line(line_no, bytes, "contains non-UTF-8 bytes"))?;
    let line = raw.trim_end_matches(['\r', '\n']).trim_matches(is_blank);
    if line.is_empty() {
        return Ok(None);
    }
    if line.contains('\r') {
        return Err(invalid_line(line_no, bytes, "contains a carriage return"));
    }
    let weight_start = line
        .rfind(is_blank)
        .map(|index| index + 1)
        .ok_or_else(|| invalid_line(line_no, bytes, "has no numeric weight"))?;
    let stack = line[..weight_start - 1].trim_end_matches(is_blank);
    let weight_text = &line[weight_start..];
    if stack.is_empty() {
        return Err(invalid_line(line_no, bytes, "has an empty stack"));
    }
    let weight_error = |code: &'static str, reason: &str| {
        ApiError::new(
            code,
            format!("line {line_no} has {reason}"),
            serde_json::json!({"line": line_no, "preview": preview(bytes)}),
            "Use prof-mcp capture or provide a valid folded stack file.",
        )
    };
    if weight_text.is_empty() || !weight_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(weight_error("invalid_weight", "invalid weight"));
    }
    let weight = weight_text
        .parse::<u64>()
        .map_err(|_| weight_error("weight_overflow", "weight above the u64 limit"))?;
    if weight == 0 {
        return Err(ApiError::new(
            "invalid_weight",
            format!("line {line_no} has zero weight"),
            serde_json::json!({"line": line_no, "preview": preview(bytes)}),
            "Folded stack weights must be positive.",
        ));
    }
    let frames: Vec<_> = stack
        .split(';')
        .take(max_depth.saturating_add(1))
        .map(|frame| frame.trim_matches(is_blank))
        .collect();
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

/// ASCII whitespace plus the vertical tab that `is_ascii_whitespace` omits.
fn is_blank(c: char) -> bool {
    c.is_ascii_whitespace() || c == '\u{b}'
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
        "Use prof-mcp capture or provide a valid folded stack file.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_spaces_and_depth() {
        assert_eq!(
            parse_line(7, b"root;with spaces  \t37\r\n", 2).unwrap(),
            Some((vec!["root", "with spaces"], 37))
        );
        let error = parse_line(7, b"root;with spaces 37\n", 1).unwrap_err();
        assert_eq!(error.code, "stack_too_deep");
        assert_eq!(error.details["line"], 7);
        assert!(parse_line(8, b" \t\r\n", 1).unwrap().is_none());
    }

    #[test]
    fn trim_surrounding_whitespace() {
        assert_eq!(
            parse_line(1, b"  root; leaf 5\n", 2).unwrap(),
            Some((vec!["root", "leaf"], 5))
        );
        assert_eq!(
            parse_line(1, b"root\x0b5\n", 3).unwrap(),
            Some((vec!["root"], 5))
        );
        assert_eq!(
            parse_line(1, b"a; ;b 1\n", 2).unwrap_err().code,
            "invalid_folded_line"
        );
        assert_eq!(
            parse_line(1, b"a +5\n", 2).unwrap_err().code,
            "invalid_weight"
        );
    }

    #[test]
    fn reject_carriage_return_terminators() {
        let error = parse_line(1, b"main 1\rfoo 2\r", 2).unwrap_err();
        assert_eq!(error.code, "invalid_folded_line");
        assert!(error.message.contains("carriage return"));
    }

    #[test]
    fn reject_invalid_utf8() {
        let error = parse_line(9, b"root;\xff\x1b 1\n", 2).unwrap_err();
        assert_eq!(error.code, "invalid_folded_line");
        assert_eq!(error.details["line"], 9);
        let preview = error.details["preview"].as_str().unwrap();
        assert!(preview.contains('�'));
        assert!(!preview.chars().any(char::is_control));
    }
}
