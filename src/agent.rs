use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// The entire agent loop. The model and tool implementations live outside it.
pub fn run_turn(
    messages: &mut Vec<Value>,
    input: &str,
    max_rounds: usize,
    mut prompt_llm: impl FnMut(&[Value]) -> Result<Vec<Value>>,
    mut call_tool: impl FnMut(&str, &str) -> Result<Value>,
    mut print_text: impl FnMut(&str),
) -> Result<()> {
    messages.push(json!({"role": "user", "content": input}));
    for _ in 0..max_rounds {
        let output = prompt_llm(messages)?;
        // Keep ALL output items, including encrypted reasoning, before tool results.
        messages.extend(output.iter().cloned());
        let mut calls = 0;
        for item in output {
            if item["type"] == "message"
                && let Some(content) = item["content"].as_array()
            {
                for part in content {
                    if let Some(text) = part["text"].as_str().or(part["refusal"].as_str()) {
                        print_text(text);
                    }
                }
            }
            if item["type"] != "function_call" {
                continue;
            }
            calls += 1;
            let id = item["call_id"]
                .as_str()
                .context("Function call missing call_id")?;
            let name = item["name"]
                .as_str()
                .context("Function call missing name")?;
            let args = item["arguments"]
                .as_str()
                .context("Function call missing arguments")?;
            // Errors are results too: every requested call receives its own call_id.
            let result = match call_tool(name, args) {
                Ok(value) => value,
                Err(error) => json!({"isError": true, "error": error.to_string()}),
            };
            messages.push(json!({"type": "function_call_output", "call_id": id,
                "output": serde_json::to_string(&result)?}));
        }
        if calls == 0 {
            return Ok(());
        }
    }
    bail!(
        "Stopped after {max_rounds} model rounds. Tool results are saved; enter another message to continue."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_tools_breaks_and_next_turn_keeps_history() {
        let mut history = vec![];
        for input in ["hello", "again"] {
            run_turn(&mut history, input, 3, |_| Ok(vec![json!({"type":"message", "role":"assistant", "content":[{"type":"output_text","text":"hi"}]})]), |_,_| panic!("no tools expected"), |_|{}).unwrap();
        }
        assert_eq!(history.len(), 4);
    }
    #[test]
    fn multiple_calls_errors_and_reasoning_keep_ids_and_order() {
        let mut history = vec![];
        let mut round = 0;
        run_turn(
            &mut history,
            "test",
            3,
            |messages| {
                round += 1;
                if round == 1 {
                    Ok(vec![
                        json!({"type":"reasoning", "encrypted_content":"opaque"}),
                        json!({"type":"function_call","call_id":"a","name":"one","arguments":"{}"}),
                        json!({"type":"function_call","call_id":"b","name":"two","arguments":"{}"}),
                    ])
                } else {
                    assert_eq!(messages[4]["call_id"], "a");
                    assert_eq!(messages[5]["call_id"], "b");
                    Ok(vec![])
                }
            },
            |name, _| {
                if name == "two" {
                    bail!("tool unavailable")
                }
                Ok(json!({"value":1}))
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(round, 2);
        assert!(
            history[5]["output"]
                .as_str()
                .unwrap()
                .contains("tool unavailable")
        );
        assert_eq!(history[1]["encrypted_content"], "opaque");
    }
    #[test]
    fn round_limit_finishes_all_pending_tool_results() {
        let mut history = vec![];
        let result = run_turn(
            &mut history,
            "x",
            1,
            |_| {
                Ok(vec![
                    json!({"type":"function_call","call_id":"a","name":"x","arguments":"{}"}),
                ])
            },
            |_, _| Ok(json!(true)),
            |_| {},
        );
        assert!(result.is_err());
        assert_eq!(history.last().unwrap()["type"], "function_call_output");
    }
}
