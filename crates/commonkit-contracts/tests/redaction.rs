use commonkit_contracts::assert_no_embedded_secrets;
use serde_json::json;

#[test]
fn rejects_embedded_secrets_and_accepts_references() {
    for unsafe_value in [
        json!({"nested": {"accessToken": "sk-live-literal"}}),
        json!("Authorization: Bearer abcdefghijklmnop"),
        json!("https://alice:hunter2@example.com/api"),
        json!("env = { API_KEY = \"literal-value\" }"),
        json!("headers = { \"X-API-Key\" = \"literal-value\" }"),
    ] {
        assert!(
            assert_no_embedded_secrets(&unsafe_value).is_err(),
            "{unsafe_value}"
        );
    }

    for safe_value in [
        json!({"api_token": "${API_TOKEN}"}),
        json!({"api_token": "env://API_TOKEN"}),
        json!({"credential": "bws://commonkit/api-token"}),
        json!({"token_budget": 4096}),
        json!("env = { API_KEY = \"${API_KEY}\" }"),
    ] {
        assert_no_embedded_secrets(&safe_value).expect("safe reference");
    }
}
