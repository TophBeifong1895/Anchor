//! Invocation-local completion acknowledgement. Goose owns the tool batch and
//! turn boundary; Anchor only tracks receipts it actually issued in that batch.

use std::collections::HashSet;

use rmcp::model::Meta;
use serde_json::Value;

const BATCH_ID: &str = "agent-tool-call-batch-id";
const BATCH_SIZE: &str = "agent-tool-call-batch-size";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ToolBatch {
    id: String,
    size: u64,
}

impl ToolBatch {
    /// These fields are overwritten by the pinned Goose adapter, never supplied
    /// by the model. Conflicting request/context metadata fails closed.
    pub(super) fn from_meta(meta: Option<&Meta>, context: &Meta) -> Option<Self> {
        let has_batch = |meta: &Meta| {
            meta.0.keys().any(|key| {
                key.eq_ignore_ascii_case(BATCH_ID) || key.eq_ignore_ascii_case(BATCH_SIZE)
            })
        };
        let meta = match meta.filter(|meta| has_batch(meta)) {
            Some(meta) if has_batch(context) && meta.0 != context.0 => return None,
            Some(meta) => meta,
            None => context,
        };
        let unique = |name: &str| {
            let mut values = meta
                .0
                .iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case(name));
            let value = values.next()?.1;
            values.next().is_none().then_some(value)
        };
        let id = unique(BATCH_ID)?.as_str()?;
        if id.is_empty() || id.len() > 512 || id.chars().any(char::is_control) {
            return None;
        }
        let size = unique(BATCH_SIZE)?.as_u64()?;
        (size > 0).then(|| Self {
            id: id.into(),
            size,
        })
    }
}

#[derive(Default)]
pub(super) struct NativeCompletion {
    latest: Option<BusinessBatch>,
    retired: HashSet<String>,
    candidate: Option<Candidate>,
}

struct BusinessBatch {
    batch: ToolBatch,
    receipts: HashSet<String>,
}

struct Candidate {
    output: Value,
    receipt: Option<String>,
}

impl NativeCompletion {
    pub(super) fn begin_business(&mut self, batch: &ToolBatch) -> Result<(), String> {
        if self.candidate.is_some() {
            return Err("tools after node completion are refused".into());
        }
        if self.retired.contains(&batch.id) {
            return Err("tools from an earlier Goose batch are refused".into());
        }
        if let Some(latest) = &self.latest
            && latest.batch.id == batch.id
        {
            return if latest.batch == *batch {
                Ok(())
            } else {
                Err("inconsistent Goose tool batch metadata".into())
            };
        }
        if let Some(previous) = self.latest.take() {
            self.retired.insert(previous.batch.id);
        }
        self.latest = Some(BusinessBatch {
            batch: batch.clone(),
            receipts: HashSet::new(),
        });
        Ok(())
    }

    pub(super) fn record_receipt(&mut self, receipt: String) {
        self.latest
            .as_mut()
            .expect("business batch is checked before execution")
            .receipts
            .insert(receipt);
    }

    pub(super) fn submit(
        &mut self,
        batch: &ToolBatch,
        output: Value,
        receipt: Option<&Value>,
        recovery_requires_observation: bool,
    ) -> Result<Value, String> {
        if batch.size != 1
            || self
                .latest
                .as_ref()
                .is_some_and(|latest| latest.batch.id == batch.id)
            || self.retired.contains(&batch.id)
        {
            return Err(
                "final_result must be called alone after observing business results".into(),
            );
        }
        // Only absence or an actual string is meaningful. In particular, null
        // cannot disguise a missing acknowledgement after a restored invocation.
        let receipt = match receipt {
            Some(Value::String(value)) => Some(value.as_str()),
            None => None,
            _ => return Err(Self::receipt_error()),
        };
        if let Some(candidate) = &self.candidate {
            return if candidate.output == output && candidate.receipt.as_deref() == receipt {
                Ok(candidate.output.clone())
            } else {
                Err("a different final_result was already accepted for this invocation".into())
            };
        }
        let observed = match &self.latest {
            Some(latest) => receipt.is_some_and(|receipt| latest.receipts.contains(receipt)),
            None => !recovery_requires_observation && receipt.is_none(),
        };
        if !observed {
            return Err(Self::receipt_error());
        }
        self.candidate = Some(Candidate {
            output: output.clone(),
            receipt: receipt.map(str::to_owned),
        });
        Ok(output)
    }

    fn receipt_error() -> String {
        "final_result.observed_receipt must match an anchor_receipt returned by the most recent authorized non-completion tool batch, including discovery and read-only tools. Copy it from any result in that batch; do not guess it or repeat business operations merely to obtain a receipt. If a current receipt is unavailable, use an authorized read-only tool to inspect existing state.".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn batch(id: &str, size: u64) -> ToolBatch {
        ToolBatch {
            id: id.into(),
            size,
        }
    }

    fn output() -> Value {
        json!({"summary":"done","route":null})
    }

    #[test]
    fn any_actual_receipt_in_latest_batch_is_valid_in_either_execution_order() {
        for receipts in [["first", "second"], ["second", "first"]] {
            for selected in receipts {
                let mut state = NativeCompletion::default();
                for receipt in receipts {
                    state.begin_business(&batch("business", 2)).unwrap();
                    state.record_receipt(receipt.into());
                }
                assert_eq!(
                    state
                        .submit(&batch("final", 1), output(), Some(&json!(selected)), false)
                        .unwrap(),
                    output()
                );
            }
        }
    }

    #[test]
    fn old_batches_guesses_and_other_invocations_cannot_acknowledge_results() {
        let mut state = NativeCompletion::default();
        state.begin_business(&batch("old", 1)).unwrap();
        state.record_receipt("old-receipt".into());
        state.begin_business(&batch("current", 1)).unwrap();
        state.record_receipt("current-receipt".into());
        assert!(state.begin_business(&batch("old", 1)).is_err());
        for receipt in [json!("old-receipt"), json!("guess"), Value::Null] {
            assert!(
                state
                    .submit(&batch("final", 1), output(), Some(&receipt), false)
                    .is_err()
            );
        }
        let mut other = NativeCompletion::default();
        assert!(
            other
                .submit(
                    &batch("other-final", 1),
                    output(),
                    Some(&json!("current-receipt")),
                    false
                )
                .is_err()
        );
        assert!(
            state
                .submit(
                    &batch("final", 1),
                    output(),
                    Some(&json!("current-receipt")),
                    false
                )
                .is_ok()
        );
    }

    #[test]
    fn mixed_batches_cannot_complete_before_or_after_business() {
        let mut state = NativeCompletion::default();
        assert!(
            state
                .submit(&batch("mixed", 2), output(), None, false)
                .is_err()
        );
        state.begin_business(&batch("mixed", 2)).unwrap();
        state.record_receipt("receipt".into());
        assert!(
            state
                .submit(&batch("mixed", 2), output(), Some(&json!("receipt")), false)
                .is_err()
        );
        assert!(
            state
                .submit(&batch("mixed", 1), output(), Some(&json!("receipt")), false)
                .is_err()
        );
        assert!(
            state
                .submit(
                    &batch("separate", 1),
                    output(),
                    Some(&json!("receipt")),
                    false
                )
                .is_ok()
        );
    }

    #[test]
    fn first_completion_needs_no_receipt_but_recovery_requires_a_new_observation() {
        assert!(
            NativeCompletion::default()
                .submit(&batch("first", 1), output(), None, false)
                .is_ok()
        );
        let mut restored = NativeCompletion::default();
        for receipt in [None, Some(json!("persisted-old-receipt"))] {
            assert!(
                restored
                    .submit(&batch("restored", 1), output(), receipt.as_ref(), true)
                    .is_err()
            );
        }
        restored
            .begin_business(&batch("read-only-inspection", 1))
            .unwrap();
        restored.record_receipt("new-receipt".into());
        assert!(
            restored
                .submit(
                    &batch("final", 1),
                    output(),
                    Some(&json!("new-receipt")),
                    true
                )
                .is_ok()
        );
    }

    #[test]
    fn accepted_candidate_is_frozen_and_identical_submissions_are_idempotent() {
        let mut state = NativeCompletion::default();
        state.begin_business(&batch("business", 1)).unwrap();
        state.record_receipt("receipt".into());
        let receipt = json!("receipt");
        state
            .submit(&batch("final", 1), output(), Some(&receipt), false)
            .unwrap();
        assert_eq!(
            state
                .submit(&batch("retry", 1), output(), Some(&receipt), true)
                .unwrap(),
            output()
        );
        assert!(
            state
                .submit(
                    &batch("conflict", 1),
                    json!({"summary":"changed","route":null}),
                    Some(&receipt),
                    false
                )
                .is_err()
        );
        assert!(
            state
                .submit(
                    &batch("wrong-receipt", 1),
                    output(),
                    Some(&json!("another")),
                    false
                )
                .is_err()
        );
        assert!(state.begin_business(&batch("later-business", 1)).is_err());
        assert_eq!(state.candidate.as_ref().unwrap().output, output());
    }

    #[test]
    fn batch_metadata_requires_unique_valid_trusted_fields() {
        let good: Meta = serde_json::from_value(json!({BATCH_ID:"batch", BATCH_SIZE:2})).unwrap();
        let empty = Meta::default();
        assert_eq!(
            ToolBatch::from_meta(Some(&good), &empty),
            Some(batch("batch", 2))
        );
        assert_eq!(ToolBatch::from_meta(None, &good), Some(batch("batch", 2)));
        assert_eq!(
            ToolBatch::from_meta(Some(&good), &good),
            Some(batch("batch", 2))
        );
        assert_eq!(
            ToolBatch::from_meta(Some(&empty), &good),
            Some(batch("batch", 2))
        );
        for invalid in [
            json!({BATCH_ID:"batch"}),
            json!({BATCH_SIZE:1}),
            json!({BATCH_ID:"",BATCH_SIZE:1}),
            json!({BATCH_ID:"bad\n",BATCH_SIZE:1}),
            json!({BATCH_ID:7,BATCH_SIZE:1}),
            json!({BATCH_ID:"batch",BATCH_SIZE:0}),
            json!({BATCH_ID:"batch",BATCH_SIZE:-1}),
            json!({BATCH_ID:"batch",BATCH_SIZE:1.5}),
            json!({BATCH_ID:"batch",BATCH_SIZE:"1"}),
            json!({BATCH_ID:"batch",BATCH_SIZE:1,"Agent-Tool-Call-Batch-Id":"spoof"}),
            json!({BATCH_ID:"batch",BATCH_SIZE:1,"Agent-Tool-Call-Batch-Size":1}),
        ] {
            let invalid: Meta = serde_json::from_value(invalid).unwrap();
            assert!(ToolBatch::from_meta(Some(&invalid), &empty).is_none());
            assert!(ToolBatch::from_meta(Some(&invalid), &good).is_none());
        }
        assert!(ToolBatch::from_meta(None, &empty).is_none());
    }
}
