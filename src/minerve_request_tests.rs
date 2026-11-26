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

/// A bug that panics mid-turn, even holding the conversation's lock, ends the turn: minerve
/// takes prompts again instead of staying "working" for good.
#[test]
fn a_turn_that_panics_ends_and_minerve_takes_prompts_again() {
    let minerve = Arc::new(build_minerve_without_keys());
    let (ui, events) = std::sync::mpsc::channel();
    minerve.start_turn(ChatCompletionMessage::user(String::from("hi")), &ui);
    assert!(minerve.request_in_flight.load(Ordering::SeqCst));

    let turn = {
        let minerve = minerve.clone();
        let ui = ui.clone();
        crate::get_global_runtime().spawn(async move {
            let _turn_guard = TurnEndsOnPanic::new(&minerve, &ui);
            let _messages = minerve.messages.lock().unwrap();
            panic!("a bug in a turn");
        })
    };
    assert!(crate::get_global_runtime()
        .block_on(turn)
        .unwrap_err()
        .is_panic());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while minerve.request_in_flight.load(Ordering::SeqCst) {
        assert!(std::time::Instant::now() < deadline, "the turn never ended");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!minerve.messages.is_poisoned());
    assert!(minerve.messages.lock().unwrap().len() > 1);
    let notices: Vec<String> = events
        .try_iter()
        .filter_map(|event| match event {
            crate::ui::UiEvent::Notice(notice) => Some(notice),
            _ => None,
        })
        .collect();
    assert!(
        notices.iter().any(|n| n.contains("bug in minerve")),
        "{notices:?}"
    );
}

/// A history written by another provider (`/model` switched mid-session) used to fail every
/// Anthropic request locally: arguments that are not JSON, ids Anthropic refuses, an empty answer.
#[test]
fn a_history_from_another_provider_still_makes_a_valid_anthropic_request() {
    let minerve = build_minerve_without_keys();
    let mut call = ChatCompletionMessage::assistant(String::new());
    call.content = None;
    call.tool_calls = Some(vec![crate::chat::ChatCompletionToolCall {
        id: String::from("functions.bash:0"),
        call_type: String::from("function"),
        function: crate::chat::ChatCompletionFunctionCall {
            name: String::from("run_shell_command"),
            arguments: String::from(r#"{"command":"ls"#),
        },
    }]);
    let mut empty_answer = ChatCompletionMessage::assistant(String::new());
    empty_answer.content = None;
    empty_answer.tool_calls = Some(vec![]);
    let history = [
        ChatCompletionMessage::system(String::from("system")),
        ChatCompletionMessage::user(String::from("list files")),
        call,
        ChatCompletionMessage::tool_result(String::from("functions.bash:0"), String::from("a.rs")),
        empty_answer,
    ];

    let request = minerve.build_request(
        &ActiveModel::new(String::from("anthropic:claude-haiku-5-5")),
        &history,
        &McpServers::none(),
    );
    let body = crate::anthropic::to_messages_body(&request)
        .unwrap()
        .to_string();
    assert!(body.contains(r#""id":"functions_bash_0""#), "{body}");
    assert!(
        body.contains(r#""tool_use_id":"functions_bash_0""#),
        "{body}"
    );
    assert!(body.contains("[empty answer]"), "{body}");
}
