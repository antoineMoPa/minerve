#[cfg(test)]
mod tests {
    use super::*;
    use crate::handle_tool_call;
    use crate::ChatCompletionFunctionCall;
    use crate::ToolCallResult;

    #[tokio::test]
    async fn test_run_shell_command_disallowed_for_subminerve() {
        let function_call = ChatCompletionFunctionCall {
            name: "run_shell_command".to_string(),
            arguments: "{}".to_string(),
            tool_call_id: Some("subminerve".to_string()),
        };

        let result = handle_tool_call(&function_call, None, true).await;

        if let ToolCallResult::Error(err_msg) = result {
            assert!(err_msg.contains("Execution not allowed"));
        } else {
            panic!("Expected an error result but got success");
        }
    }

    #[tokio::test]
    async fn test_run_shell_command_disallowed_for_subminerve_executor() {
        let function_call = ChatCompletionFunctionCall {
            name: "run_shell_command".to_string(),
            arguments: "{}".to_string(),
            tool_call_id: Some("subminerve_executor".to_string()),
        };

        let result = handle_tool_call(&function_call, None, true).await;

        if let ToolCallResult::Error(err_msg) = result {
            assert!(err_msg.contains("Execution not allowed"));
        } else {
            panic!("Expected an error result but got success");
        }
    }

    #[tokio::test]
    async fn test_run_shell_command_disallowed_for_subminerve_qa() {
        let function_call = ChatCompletionFunctionCall {
            name: "run_shell_command".to_string(),
            arguments: "{}".to_string(),
            tool_call_id: Some("subminerve_qa".to_string()),
        };

        let result = handle_tool_call(&function_call, None, true).await;

        if let ToolCallResult::Error(err_msg) = result {
            assert!(err_msg.contains("Execution not allowed"));
        } else {
            panic!("Expected an error result but got success");
        }
    }
}