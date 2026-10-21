//! Turn one invocation's evidence into harness facts: what the host observed
//! of model turns and bridge tool calls, without their business content.

use anchor_facts::{Fact, Outcome};
use serde_json::Value;

pub(super) fn observations(
    evidence: &Value,
    started_at_ms: u64,
    ended_at_ms: u64,
    outcome: Outcome,
) -> Vec<Fact> {
    let samples = evidence["usage"]["samples"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let calls = evidence["tool_calls"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut facts = Vec::with_capacity(samples.len() + calls.len() + 1);
    for (index, sample) in samples.iter().enumerate() {
        facts.push(Fact::ModelTurn {
            index: index as u64 + 1,
            message_id: sample["message_id"].as_str().map(str::to_owned),
            observed_at_ms: sample["observed_at_ms"].as_u64(),
            elapsed_ms: sample["elapsed_ms"].as_u64(),
            time_to_first_token_ms: sample["time_to_first_token_ms"].as_u64(),
            input_tokens: sample["input_tokens"].as_u64().unwrap_or(0),
            output_tokens: sample["output_tokens"].as_u64().unwrap_or(0),
            total_tokens: sample["total_tokens"].as_u64().unwrap_or(0),
            cache_read_tokens: sample["cache_read_tokens"].as_u64(),
            cache_write_tokens: sample["cache_write_tokens"].as_u64(),
            cost_usd: sample["cost_usd"].as_f64(),
            compaction: sample["is_compaction"].as_bool().unwrap_or(false),
        });
    }
    for call in &calls {
        let ok = call["result"]["ok"].as_bool().unwrap_or(false);
        facts.push(Fact::ToolCall {
            tool: call["tool"].as_str().unwrap_or_default().to_owned(),
            tool_call_id: call["tool_call_id"].as_str().map(str::to_owned),
            model_request: call["provider_request"].as_u64().unwrap_or(0),
            arguments_shape: anchor_facts::arguments_shape(&call["arguments"]),
            path: call["arguments"]["path"]
                .as_str()
                .map(anchor_facts::path_class),
            started_at_ms: call["started_at_ms"].as_u64(),
            ended_at_ms: call["ended_at_ms"].as_u64(),
            ok,
            failure: (!ok).then(|| anchor_facts::classify_failure(&error_text(&call["result"]))),
        });
    }
    facts.push(Fact::Lifecycle {
        started_at_ms,
        ended_at_ms,
        outcome,
        model_turns: evidence["usage"]["messages"]
            .as_u64()
            .unwrap_or(samples.len() as u64),
        tool_calls: calls.len() as u64 + evidence["tool_calls_dropped"].as_u64().unwrap_or(0),
        tool_calls_dropped: evidence["tool_calls_dropped"].as_u64().unwrap_or(0),
        resumed: evidence["resume"].as_bool().unwrap_or(false),
    });
    facts
}

/// Bridge errors are text, or a JSON envelope whose `error` is text.
fn error_text(result: &Value) -> String {
    let error = &result["error"];
    match error.as_str() {
        Some(text) => serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|inner| inner["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| text.to_owned()),
        None => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_facts::Failure;
    use serde_json::json;

    #[test]
    fn evidence_becomes_turns_calls_and_a_lifecycle_without_business_values() {
        let evidence = json!({
            "resume": false,
            "tool_calls_dropped": 0,
            "usage": {"messages": 2, "samples": [
                {"input_tokens": 10, "output_tokens": 2, "total_tokens": 12, "is_compaction": false,
                 "message_id": "m1", "observed_at_ms": 100, "elapsed_ms": 40, "time_to_first_token_ms": 30,
                 "cache_read_tokens": 8, "cost_usd": 0.001},
                {"input_tokens": 20, "output_tokens": 3, "total_tokens": 23, "is_compaction": true}
            ]},
            "tool_calls": [
                {"provider_request": 1, "tool": "anchor_run", "arguments": {"command": ["find", "."]},
                 "tool_call_id": "call_1", "started_at_ms": 110, "ended_at_ms": 115,
                 "result": {"ok": false, "error": "{\"ok\":false,\"error\":\"tool failed: anchor_run not_executed: command `find` is not authorized by the host sandbox policy\",\"anchor_receipt\":\"r\"}"}},
                {"provider_request": 2, "tool": "final_result", "arguments": {"summary": "done"},
                 "result": {"ok": true, "output": {"summary": "done"}}}
            ]
        });
        let facts = observations(&evidence, 90, 200, Outcome::Completed);
        assert_eq!(facts.len(), 5);
        assert!(
            matches!(&facts[0], Fact::ModelTurn { index: 1, message_id: Some(id), cache_read_tokens: Some(8), .. } if id == "m1")
        );
        assert!(matches!(
            &facts[1],
            Fact::ModelTurn {
                index: 2,
                compaction: true,
                observed_at_ms: None,
                ..
            }
        ));
        match &facts[2] {
            Fact::ToolCall {
                tool,
                model_request,
                ok,
                failure,
                started_at_ms,
                ended_at_ms,
                tool_call_id,
                ..
            } => {
                assert_eq!(tool, "anchor_run");
                assert_eq!(*model_request, 1);
                assert!(!ok);
                assert_eq!(*failure, Some(Failure::NotAuthorized));
                assert_eq!((*started_at_ms, *ended_at_ms), (Some(110), Some(115)));
                assert_eq!(tool_call_id.as_deref(), Some("call_1"));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            &facts[3],
            Fact::ToolCall {
                ok: true,
                failure: None,
                ..
            }
        ));
        assert!(matches!(
            &facts[4],
            Fact::Lifecycle {
                model_turns: 2,
                tool_calls: 2,
                outcome: Outcome::Completed,
                ..
            }
        ));
        // No argument or result value is carried.
        let recorded = format!("{facts:?}");
        assert!(!recorded.contains("find"));
        assert!(!recorded.contains("sandbox policy"));
        assert!(!recorded.contains("done"));
    }
}
