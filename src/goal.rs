//! `/goal <text>`: the agent keeps taking turns until an evaluator model judges the goal reached.
//!
//! After each turn the conversation since the goal was given goes to the evaluator, which answers
//! `{"status": "reached" | "not_reached" | "stalled", "reason": "..."}`. When not reached, the reason
//! is sent back to the agent as the next prompt. The loop itself is `Minerve::pursue_goal`.

use serde::Deserialize;

use crate::minerve::{post_request_with_token_count, Minerve};
use crate::provider::{ActiveModel, Provider};
use crate::{ChatCompletionMessage, ChatCompletionMessageRole, ChatCompletionRequest, RequestMessage, UsageAccountingRequest};

/// Turns the agent takes for one goal before the loop gives up.
pub const MAX_GOAL_TURNS: usize = 30;

/// Evaluator models tried in order, for the provider of the model the agent runs on.
/// A model that fails (unavailable, error, answer that is not a verdict) hands over to the next one.
const EVALUATOR_MODEL_IDS_BY_PROVIDER: [(Provider, &[&str]); 3] = [
    (
        Provider::OpenRouter,
        &["openrouter:~openai/gpt-luna-latest", "openrouter:~anthropic/claude-haiku-latest"],
    ),
    (Provider::OpenAi, &["openai:gpt-6-luna", "openai:gpt-4o-mini"]),
    (Provider::Anthropic, &["anthropic:claude-haiku-5-5"]),
];

/// Longest tool result passed to the evaluator; longer ones keep their start.
const MAX_TOOL_RESULT_CHARS_FOR_EVALUATOR: usize = 4000;

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Reached,
    /// The agent can make more progress: the reason is sent back to it.
    NotReached,
    /// More turns would not help, e.g. the goal's rules contradict each other or the agent
    /// keeps failing the same way. Ends the loop.
    Stalled,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GoalVerdict {
    pub status: GoalStatus,
    pub reason: String,
}

/// How a goal loop ended.
pub enum GoalOutcome {
    Reached { turns: usize, evaluator_model_id: &'static str, reason: String },
    /// `MAX_GOAL_TURNS` turns ran without the goal being reached.
    NotReached,
    Stalled { turns: usize, evaluator_model_id: &'static str, reason: String },
    Cancelled,
    /// The session cost its `/budget`.
    OverBudget,
    RequestFailed { error: String },
    EvaluatorFailed { errors: String },
}

impl GoalOutcome {
    /// Line shown in the chat when the loop ends.
    pub fn notice(&self) -> String {
        match self {
            GoalOutcome::Reached { turns, evaluator_model_id, reason } => {
                format!("Goal reached after {} turn(s), judged by {}: {}", turns, evaluator_model_id, reason)
            }
            GoalOutcome::NotReached => {
                format!("Goal stopped after {} turns without being reached.", MAX_GOAL_TURNS)
            }
            GoalOutcome::Stalled { turns, evaluator_model_id, reason } => {
                format!("Goal stalled after {} turn(s), judged by {}: {}", turns, evaluator_model_id, reason)
            }
            GoalOutcome::Cancelled => String::from("Goal cancelled."),
            GoalOutcome::OverBudget => String::from("Goal paused at the budget: raise it with /budget <usd>, then /goal again to go on."),
            GoalOutcome::RequestFailed { error } => format!("Goal stopped: a request failed: {}", error),
            GoalOutcome::EvaluatorFailed { errors } => {
                format!("Goal stopped: no evaluator model gave a verdict.\n{}", errors)
            }
        }
    }
}

pub fn evaluator_model_ids(provider: Provider) -> &'static [&'static str] {
    EVALUATOR_MODEL_IDS_BY_PROVIDER
        .iter()
        .find(|(candidate, _)| *candidate == provider)
        .map(|(_, ids)| *ids)
        .expect("provider without goal evaluator models")
}

/// First prompt of a goal, sent as the user message that starts the loop.
pub fn start_prompt(goal: &str) -> String {
    include_str!("../prompts/GOAL_START_PROMPT.txt").replace("{goal}", goal)
}

/// Prompt sent after a turn the evaluator judged not done.
pub fn continue_prompt(goal: &str, reason: &str) -> String {
    include_str!("../prompts/GOAL_CONTINUE_PROMPT.txt")
        .replace("{reason}", reason)
        .replace("{goal}", goal)
}

/// The goal and the conversation since it was given, as text for the evaluator.
fn evaluator_input(goal: &str, messages_since_goal: &[ChatCompletionMessage]) -> String {
    let mut text = format!("GOAL:\n{}\n\nCONVERSATION SINCE THE GOAL WAS GIVEN:\n", goal);
    for message in messages_since_goal {
        let content = message.content.as_deref().unwrap_or_default();
        match message.role {
            ChatCompletionMessageRole::System => {}
            ChatCompletionMessageRole::User => text.push_str(&format!("\n[user]\n{}\n", content)),
            ChatCompletionMessageRole::Assistant => {
                if !content.is_empty() {
                    text.push_str(&format!("\n[agent]\n{}\n", content));
                }
                for tool_call in message.tool_calls.iter().flatten() {
                    text.push_str(&format!(
                        "\n[agent tool call] {}({})\n",
                        tool_call.function.name, tool_call.function.arguments
                    ));
                }
            }
            ChatCompletionMessageRole::Tool => {
                let shown: String = content.chars().take(MAX_TOOL_RESULT_CHARS_FOR_EVALUATOR).collect();
                let cut = if shown.len() < content.len() { "\n...[cut]" } else { "" };
                text.push_str(&format!("\n[tool result]\n{}{}\n", shown, cut));
            }
        }
    }
    text
}

fn parse_verdict(answer: &str) -> Result<GoalVerdict, String> {
    serde_json::from_str(answer.trim())
        .map_err(|err| format!("answer is not a goal verdict ({}): {}", err, answer))
}

/// Asks one evaluator model for its verdict.
async fn ask_evaluator(minerve: &Minerve, model: &ActiveModel, input: &str) -> Result<GoalVerdict, String> {
    let (url, api_key) = minerve.endpoint_for(model)?;
    let messages = [
        ChatCompletionMessage::system(String::from(include_str!(
            "../prompts/GOAL_EVALUATOR_SYSTEM_PROMPT.txt"
        ))),
        ChatCompletionMessage::user(input.to_string()),
    ];
    let request = ChatCompletionRequest {
        model: model.api_model.clone(),
        messages: messages.iter().map(RequestMessage::from_message).collect::<Result<_, _>>()?,
        tools: None,
        usage: (model.provider == Provider::OpenRouter).then_some(UsageAccountingRequest { include: true }),
    };
    let response = post_request_with_token_count(
        &minerve.client,
        model.provider,
        &url,
        &api_key,
        request,
        minerve.token_counter.clone(),
    )
    .await?;
    let answer = response
        .choices
        .into_iter()
        .next()
        .expect("chat completion response without choices")
        .message
        .content
        .ok_or_else(|| String::from("evaluator answered without text"))?;
    parse_verdict(&answer)
}

/// Verdict of the first evaluator model of `provider` that gives one, with that model's id.
/// Fails with every model's error when none does.
pub async fn evaluate(
    minerve: &Minerve,
    provider: Provider,
    goal: &str,
    messages_since_goal: &[ChatCompletionMessage],
) -> Result<(GoalVerdict, &'static str), String> {
    let input = evaluator_input(goal, messages_since_goal);
    let mut errors = Vec::new();
    for model_id in evaluator_model_ids(provider) {
        let model = ActiveModel::new(model_id.to_string());
        match ask_evaluator(minerve, &model, &input).await {
            Ok(verdict) => return Ok((verdict, model_id)),
            Err(err) => errors.push(format!("{}: {}", model_id, err)),
        }
    }
    Err(errors.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_provider_has_evaluator_models() {
        for (provider, ids) in EVALUATOR_MODEL_IDS_BY_PROVIDER {
            assert!(!ids.is_empty());
            for id in ids {
                assert_eq!(ActiveModel::new(id.to_string()).provider, provider);
            }
        }
    }

    #[test]
    fn verdict_is_parsed_strictly() {
        assert_eq!(
            parse_verdict(" {\"status\": \"not_reached\", \"reason\": \"3 lines left\"}\n").unwrap(),
            GoalVerdict { status: GoalStatus::NotReached, reason: String::from("3 lines left") }
        );
        assert_eq!(
            parse_verdict("{\"status\": \"stalled\", \"reason\": \"x\"}").unwrap().status,
            GoalStatus::Stalled
        );
        assert!(parse_verdict("```json\n{\"status\": \"reached\", \"reason\": \"x\"}\n```").is_err());
        assert!(parse_verdict("{\"status\": \"done\", \"reason\": \"x\"}").is_err());
        assert!(parse_verdict("{\"status\": \"reached\"}").is_err());
    }

    #[test]
    fn evaluator_input_shows_tool_calls_and_results() {
        let mut call = ChatCompletionMessage::assistant(String::new());
        call.tool_calls = Some(vec![crate::ChatCompletionToolCall {
            id: String::from("1"),
            call_type: String::from("function"),
            function: crate::chat::ChatCompletionFunctionCall {
                name: String::from("read_file"),
                arguments: String::from("{\"path\":\"a.txt\"}"),
            },
        }]);
        let messages = vec![
            ChatCompletionMessage::user(start_prompt("empty a.txt")),
            call,
            ChatCompletionMessage::tool_result(String::from("1"), String::from("")),
        ];
        let input = evaluator_input("empty a.txt", &messages);
        assert!(input.starts_with("GOAL:\nempty a.txt\n"));
        assert!(input.contains("[agent tool call] read_file({\"path\":\"a.txt\"})"));
        assert!(input.contains("[tool result]"));
    }
}
