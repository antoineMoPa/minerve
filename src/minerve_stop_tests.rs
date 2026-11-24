//! A stop while a tool call runs.

use std::time::{Duration, Instant};

use super::*;
use crate::chat::ChatCompletionFunctionCall;

fn build_shell_tool_call(id: &str, command: &str) -> ChatCompletionToolCall {
    ChatCompletionToolCall {
        id: id.to_string(),
        call_type: String::from("function"),
        function: ChatCompletionFunctionCall {
            name: String::from("run_shell_command"),
            arguments: serde_json::json!({ "command": command }).to_string(),
        },
    }
}

#[tokio::test]
async fn a_stop_ends_the_running_tool_call_and_answers_every_call() {
    let tool_calls = [
        build_shell_tool_call("1", "sleep 30"),
        build_shell_tool_call("2", "echo never"),
    ];
    let cancel_requested = Arc::new(AtomicBool::new(false));
    let cancel_notify = Arc::new(Notify::new());
    {
        // What `Minerve::cancel_turn` does, a moment into the first call.
        let (cancel_requested, cancel_notify) = (cancel_requested.clone(), cancel_notify.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            cancel_requested.store(true, Ordering::SeqCst);
            cancel_notify.notify_waiters();
        });
    }

    let mut answers = Vec::new();
    let started = Instant::now();
    let should_continue = run_tool_calls(
        &tool_calls,
        &McpServers::none(),
        None,
        false,
        &cancel_requested,
        &cancel_notify,
        &mut |message| answers.push(message.content.unwrap_or_default()),
    )
    .await;

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the stop waited for sleep"
    );
    assert!(!should_continue);
    assert_eq!(answers.len(), 2, "every tool call is answered");
    assert!(answers[0].starts_with("Stopped:"), "{}", answers[0]);
    assert!(answers[1].starts_with("Skipped:"), "{}", answers[1]);
}

#[tokio::test]
async fn a_tool_call_that_fails_is_answered_with_its_error_and_the_next_calls_still_run() {
    let unknown_tool_call = ChatCompletionToolCall {
        id: String::from("1"),
        call_type: String::from("function"),
        function: ChatCompletionFunctionCall {
            name: String::from("no_such_tool"),
            arguments: String::from("{}"),
        },
    };
    let tool_calls = [unknown_tool_call, build_shell_tool_call("2", "echo after")];

    let mut answers = Vec::new();
    let should_continue = run_tool_calls(
        &tool_calls,
        &McpServers::none(),
        None,
        false,
        &AtomicBool::new(false),
        &Notify::new(),
        &mut |message| answers.push(message.content.unwrap_or_default()),
    )
    .await;

    assert!(should_continue, "the model gets to read the failure");
    assert!(answers[0].starts_with("Error:"), "{}", answers[0]);
    assert!(answers[0].contains("no_such_tool"), "{}", answers[0]);
    assert_eq!(answers[1], "after\n");
}
