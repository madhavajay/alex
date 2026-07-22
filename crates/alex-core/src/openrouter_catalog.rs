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
    fn parses_realistic_openrouter_catalog_with_extra_fields() {
        let payload: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/openrouter_models.json")).unwrap();
        assert_eq!(
            parse_models_response(&payload),
            vec![
                "anthropic/claude-sonnet-4",
                "openai/gpt-5",
                "google/gemini-2.5-pro",
            ]
        );
    }

    #[test]
    fn model_catalog_edge_cases_are_ignored_without_reordering_valid_ids() {
        let cases = [
            ("empty list", json!({"data": []}), Vec::<String>::new()),
            ("missing data", json!({}), Vec::new()),
            ("wrong data type", json!({"data": {"id": "m"}}), Vec::new()),
            (
                "missing and invalid ids",
                json!({
                    "data": [
                        {"name": "missing"},
                        {"id": null},
                        {"id": 7},
                        {"id": "   "},
                        {"id": " valid/model ", "unexpected": {"nested": true}},
                        {"id": "valid/model"},
                        {"id": "second/model", "extra": [1, 2, 3]}
                    ],
                    "unexpected_top_level": true
                }),
                vec!["valid/model".to_string(), "second/model".to_string()],
            ),
        ];

        for (name, payload, expected) in cases {
            assert_eq!(parse_models_response(&payload), expected, "{name}");
        }
    }
}
