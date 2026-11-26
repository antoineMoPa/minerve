//! Running the tool calls of an answer: each is answered, stopped with the turn, skipped by
//! the user, or failed with its error.

use super::*;

/// How long a tool call runs before the user may skip it (`skip_tool_call`).
pub(super) const SKIPPABLE_AFTER: std::time::Duration = std::time::Duration::from_secs(8);

/// What can end a tool call early: the user stopping the turn (`cancel_requested`, woken by
/// `cancel_notify`) or skipping the call (`skip_notify`, see `Minerve::skip_tool_call`).
pub(super) struct ToolCallStops<'a> {
    pub cancel_requested: &'a AtomicBool,
    pub cancel_notify: &'a Notify,
    pub skip_notify: &'a Notify,
}

/// Every tool call of an assistant message needs a tool message answering it,
/// otherwise OpenRouter rejects the next request.
/// A call that fails is answered with its error, and the calls after it still run: the model
/// reads the failure and goes on. Only a stop ends the run.
/// Returns whether the conversation should continue with another request.
pub(super) async fn run_tool_calls(
    tool_calls: &[ChatCompletionToolCall],
    mcp: &McpServers,
    ui: Option<UiSender>,
    is_headless: bool,
    stops: ToolCallStops<'_>,
    on_tool_message: &mut impl FnMut(ChatCompletionMessage),
) -> bool {
    let ToolCallStops {
        cancel_requested,
        cancel_notify,
        skip_notify,
    } = stops;
    let mut should_continue = true;
    for tool_call in tool_calls {
        // Cancelled calls are still answered, so the history stays valid for the next prompt.
        if cancel_requested.load(Ordering::SeqCst) {
            on_tool_message(ChatCompletionMessage::tool_result(
                tool_call.id.clone(),
                String::from("Skipped: the turn was cancelled."),
            ));
            should_continue = false;
            continue;
        }
        // Registered before the call starts, so a stop while the tool runs ends it: the call's
        // future is dropped, and the command it was waiting for is killed.
        let cancelled = cancel_notify.notified();
        tokio::pin!(cancelled);
        cancelled.as_mut().enable();
        let skipped = skip_notify.notified();
        tokio::pin!(skipped);
        skipped.as_mut().enable();
        let call = handle_tool_call(tool_call, mcp, ui.clone(), is_headless);
        tokio::pin!(call);
        let skippable_after = tokio::time::sleep(SKIPPABLE_AFTER);
        tokio::pin!(skippable_after);
        let mut is_skippable = false;
        let result = loop {
            tokio::select! {
                result = &mut call => break Some(result),
                _ = &mut cancelled => {
                    on_tool_message(ChatCompletionMessage::tool_result(
                        tool_call.id.clone(),
                        String::from("Stopped: the user stopped the turn while this tool call was running."),
                    ));
                    should_continue = false;
                    break None;
                }
                _ = &mut skipped, if is_skippable => {
                    on_tool_message(ChatCompletionMessage::tool_result(
                        tool_call.id.clone(),
                        format!(
                            "Skipped: the user stopped this tool call after more than {}s; its output is lost. \
                             The turn goes on: if it hangs, try another way.",
                            SKIPPABLE_AFTER.as_secs()
                        ),
                    ));
                    break None;
                }
                _ = &mut skippable_after, if !is_skippable => {
                    is_skippable = true;
                    if let Some(ui) = &ui {
                        let _ = ui.send(crate::ui::UiEvent::ToolCallSkippable(true));
                    }
                }
            }
        };
        if is_skippable {
            if let Some(ui) = &ui {
                let _ = ui.send(crate::ui::UiEvent::ToolCallSkippable(false));
            }
        }
        let Some(result) = result else { continue };
        match result {
            ToolCallResult::Success(mut msg) => {
                msg.content = msg.content.map(cap_tool_output);
                on_tool_message(msg)
            }
            ToolCallResult::Error(err) => {
                on_tool_message(ChatCompletionMessage::tool_result(
                    tool_call.id.clone(),
                    format!("Error: {}", err),
                ));
            }
        }
    }
    should_continue
}

pub async fn handle_tool_call(
    tool_call: &ChatCompletionToolCall,
    mcp: &McpServers,
    ui: Option<UiSender>,
    is_headless: bool,
) -> ToolCallResult {
    let settings = crate::tools::ExecuteCommandSettings { is_headless };
    let registry = get_tool_registry();
    let tool_name = &tool_call.function.name;
    let args_str = &tool_call.function.arguments;

    if let Some(ui) = &ui {
        send_status(ui, describe_running_tool_call(tool_name, args_str));
    }
    if let Some(mcp_result) = mcp.call(tool_name, args_str).await {
        return match mcp_result {
            Ok(output) => ToolCallResult::Success(ChatCompletionMessage::tool_result(
                tool_call.id.clone(),
                output,
            )),
            Err(err) => ToolCallResult::Error(format!("MCP tool '{}': {}", tool_name, err)),
        };
    }

    if let Some(tool) = registry.get(tool_name.as_str()) {
        // Parse as generic JSON value first, then convert all values to strings
        let args: HashMap<String, String> =
            match serde_json::from_str::<serde_json::Value>(args_str) {
                Ok(serde_json::Value::Object(map)) => map
                    .into_iter()
                    .map(|(k, v)| {
                        let string_value = match v {
                            serde_json::Value::String(s) => s,
                            serde_json::Value::Number(n) => n.to_string(),
                            serde_json::Value::Bool(b) => b.to_string(),
                            _ => v.to_string(),
                        };
                        (k, string_value)
                    })
                    .collect(),
                _ => HashMap::new(),
            };

        let (result, images) = tool.run_with_images(args, settings).await;

        if let Some(ui) = &ui {
            send_status(ui, format!("Reading tool result: {}", tool_name));
        }

        ToolCallResult::Success(ChatCompletionMessage {
            images,
            ..ChatCompletionMessage::tool_result(tool_call.id.clone(), result)
        })
    } else {
        ToolCallResult::Error(format!("Function '{}' not found", tool_name))
    }
}
