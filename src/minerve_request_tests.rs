//! What a session's request carries besides its messages.

use super::*;

fn build_request_json_for_model(minerve: &Minerve, model_id: &str) -> serde_json::Value {
    let model = ActiveModel::new(String::from(model_id));
    let history = [ChatCompletionMessage::user(String::from("hi"))];
    let request = minerve
        .build_request(&model, &history, &McpServers::none())
        .unwrap();
    serde_json::to_value(request).unwrap()
}

#[test]
fn only_openrouter_requests_carry_the_session_id() {
    let connection = OpenRouterConnection {
        base_url: String::from("http://unused"),
        api_keys: ApiKeys::default(),
        model: String::from("x/one"),
    };
    let minerve = Minerve::new(connection, SessionStart::New, &[], None);

    let openrouter = build_request_json_for_model(&minerve, "x/one");
    assert_eq!(openrouter["session_id"], minerve.current_session_id());

    // OpenAI and Anthropic reject fields they do not know.
    for model_id in ["openai:gpt-5", "anthropic:claude-haiku-5-5"] {
        let other = build_request_json_for_model(&minerve, model_id);
        assert!(other.get("session_id").is_none(), "{model_id}");
    }

    minerve.start_new_session();
    let after = build_request_json_for_model(&minerve, "x/one");
    assert_eq!(after["session_id"], minerve.current_session_id());
    assert_ne!(after["session_id"], openrouter["session_id"]);
}
