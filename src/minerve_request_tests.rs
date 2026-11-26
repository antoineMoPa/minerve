//! What a session's request carries besides its messages.

use super::*;

fn build_request_json_for_model(minerve: &Minerve, model_id: &str) -> serde_json::Value {
    let model = ActiveModel::new(String::from(model_id));
    let history = [ChatCompletionMessage::user(String::from("hi"))];
    let request = minerve.build_request(&model, &history, &McpServers::none());
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

fn build_minerve_without_keys() -> Minerve {
    let connection = OpenRouterConnection {
        base_url: String::from("http://unused"),
        api_keys: ApiKeys::default(),
        model: String::from("x/one"),
    };
    Minerve::new(connection, SessionStart::New, &[], None)
}

/// A history whose `view_image` result points at a file deleted since, like a session saved
/// before images were copied: the next text prompt still reaches the model.
#[test]
fn a_deleted_image_from_an_earlier_tool_call_does_not_block_the_next_prompt() {
    let minerve = build_minerve_without_keys();
    let mut call = ChatCompletionMessage::assistant(String::new());
    call.tool_calls = Some(vec![]);
    let mut viewed = ChatCompletionMessage::tool_result(String::from("1"), String::from("opened"));
    viewed.images = vec![std::path::PathBuf::from("/deleted/end-check.png")];
    let history = [
        ChatCompletionMessage::user(String::from("check the render")),
        call,
        viewed,
        ChatCompletionMessage::user(String::from("test")),
    ];

    let request = minerve.build_request(
        &ActiveModel::new(String::from("x/one")),
        &history,
        &McpServers::none(),
    );
    let json = serde_json::to_string(&request).unwrap();
    assert!(
        json.contains("[Image not shown: cannot read image /deleted/end-check.png"),
        "{json}"
    );
    let last = serde_json::to_value(request.messages.last().unwrap()).unwrap();
    assert_eq!(last["content"], "test");
}

/// A prompt that cannot be sent gets a notice, not an assistant message the model would later
/// read as its own reply.
#[tokio::test]
async fn a_prompt_that_cannot_be_sent_gets_a_notice_and_no_assistant_message() {
    static NO_MCP_SERVERS: std::sync::Once = std::sync::Once::new();
    NO_MCP_SERVERS.call_once(|| crate::mcp::start_connecting(Default::default()));
    let minerve = build_minerve_without_keys();
    let (ui, events) = std::sync::mpsc::channel();
    minerve.add_prompt(ChatCompletionMessage::user(String::from("test")), &ui);

    assert!(minerve.run_turn(&ui, false).await.is_err());

    let messages = minerve.messages.lock().unwrap();
    assert!(
        !messages
            .iter()
            .any(|m| matches!(m.role, ChatCompletionMessageRole::Assistant)),
        "an assistant message was added"
    );
    let notices: Vec<String> = events
        .try_iter()
        .filter_map(|event| match event {
            crate::ui::UiEvent::Notice(notice) => Some(notice),
            _ => None,
        })
        .collect();
    assert!(
        notices.iter().any(|n| n.starts_with("Not sent: ")),
        "{notices:?}"
    );
}
