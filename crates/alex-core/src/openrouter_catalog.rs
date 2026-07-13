//! OpenRouter's dynamic `/api/v1/models` response.

use serde_json::Value;

/// Return usable model IDs in response order, ignoring malformed and duplicate entries.
///
/// Callers add their local routing prefix (for example `openrouter/`) themselves so
/// this parser stays faithful to OpenRouter's wire response.
pub fn parse_models_response(payload: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    for entry in payload["data"].as_array().into_iter().flatten() {
        let Some(id) = entry["id"]
            .as_str()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if !ids.iter().any(|known| known == id) {
            ids.push(id.to_string());
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::parse_models_response;
    use serde_json::json;

    #[test]
    fn parses_openrouter_models_payload() {
        let payload = json!({
            "data": [
                {"id": "anthropic/claude-3.5-sonnet", "name": "Claude"},
                {"id": "openai/gpt-4o"},
                {"id": "meta-llama/llama-3.1-70b-instruct"},
                {"id": "openai/gpt-4o"},
                {"id": ""},
                {"name": "missing id"}
            ]
        });
        assert_eq!(
            parse_models_response(&payload),
            vec![
                "anthropic/claude-3.5-sonnet",
                "openai/gpt-4o",
                "meta-llama/llama-3.1-70b-instruct",
            ]
        );
    }
}
