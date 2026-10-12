//! Conversation projection from existing Turn facts; no separate history is persisted.
use anchor_platform_session::{Question, QuestionAction, SessionStore};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn messages(
    sessions: &SessionStore,
    owner: &str,
    session: &str,
) -> Result<Vec<Value>, String> {
    let turns = sessions
        .list_turns(owner, session)
        .map_err(|_| "Pilot history unavailable")?;
    let mut messages = Vec::new();
    for turn in turns.into_iter().rev() {
        if let Some(prompt) = turn.prompt {
            messages.push(json!({"role":"user","text":prompt}));
        }
        let events = sessions
            .turn_events(owner, session, &turn.id, 0)
            .map_err(|_| "Pilot history unavailable")?;
        let mut text = String::new();
        let mut positions = BTreeMap::<String, usize>::new();
        let mut answered = BTreeSet::new();
        for event in events {
            let kind = event.data["type"].as_str().unwrap_or_default();
            if kind == "text-delta" {
                if let Some(delta) = event.data["delta"].as_str() {
                    text.push_str(delta);
                }
                continue;
            }
            if !matches!(
                kind,
                "question" | "question-answered" | "question-interrupted"
            ) {
                continue;
            }
            let Ok(question) = serde_json::from_value::<Question>(event.data["question"].clone())
            else {
                continue;
            };
            if question.session != session || question.turn != turn.id {
                continue;
            }
            if let Some(position) = positions.get(&question.id) {
                // A late pending snapshot cannot undo the recorded answer or interruption.
                if messages[*position]["question"]["status"] == "pending" {
                    messages[*position]["question"] = json!(question);
                }
            } else {
                flush_text(&mut messages, &mut text);
                positions.insert(question.id.clone(), messages.len());
                messages
                    .push(json!({"role":"assistant","text":question.message,"question":question}));
            }
            if kind == "question-answered"
                && let Some(answer) = answer_text(&question)
                && answered.insert(question.id.clone())
            {
                flush_text(&mut messages, &mut text);
                messages.push(json!({"role":"user","text":answer,"question_answer":question}));
            }
        }
        flush_text(&mut messages, &mut text);
    }
    Ok(messages)
}

fn flush_text(messages: &mut Vec<Value>, text: &mut String) {
    if !text.is_empty() {
        messages.push(json!({"role":"assistant","text":std::mem::take(text)}));
    }
}

fn answer_text(question: &Question) -> Option<String> {
    let answer = question.answer.as_ref()?;
    match answer.action {
        QuestionAction::Decline => Some("已拒绝".into()),
        QuestionAction::Cancel => Some("已取消".into()),
        QuestionAction::Accept => {
            let fields = answer.content.as_ref().and_then(Value::as_object);
            let lines = fields
                .into_iter()
                .flatten()
                .map(|(name, value)| {
                    let title = question.requested_schema["properties"][name]["title"]
                        .as_str()
                        .unwrap_or(name);
                    let value = match value {
                        Value::Bool(true) => "是".into(),
                        Value::Bool(false) => "否".into(),
                        Value::String(value) => value.clone(),
                        value => value.to_string(),
                    };
                    format!("{title}：{value}")
                })
                .collect::<Vec<_>>();
            Some(if lines.is_empty() {
                "已回答".into()
            } else {
                lines.join("\n")
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_platform_session::{CreateSession, QuestionAnswer, TurnStatus};

    #[test]
    fn saved_questions_and_answers_stay_between_the_surrounding_replies() {
        let root = tempfile::tempdir().unwrap();
        let sessions = SessionStore::open(root.path().join("sessions.sqlite")).unwrap();
        sessions
            .create(
                "local",
                CreateSession {
                    id: Some("pilot".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let (turn, _) = sessions
            .create_turn("local", "pilot", "first", Some("重复的输入"))
            .unwrap();
        sessions
            .append_turn_event(
                "local",
                "pilot",
                &turn.id,
                json!({"type":"text-delta","delta":"回答前的说明"}),
            )
            .unwrap();
        let question = sessions.create_question("local", "pilot", &turn.id, "选择哪份报告？", json!({
            "type":"object", "properties": {"confirm":{"type":"boolean","title":"确认"},"report":{"type":"string","title":"报告"}}
        })).unwrap();
        let question = sessions
            .answer_question(
                "local",
                "pilot",
                &turn.id,
                &question.id,
                QuestionAnswer {
                    action: QuestionAction::Accept,
                    content: Some(json!({"confirm":false,"report":"季度报告"})),
                },
            )
            .unwrap();
        sessions
            .append_turn_event(
                "local",
                "pilot",
                &turn.id,
                json!({"type":"text-delta","delta":"回答后的回复"}),
            )
            .unwrap();
        // Event replay must not duplicate an answer or resurrect an earlier pending snapshot.
        sessions
            .append_turn_event(
                "local",
                "pilot",
                &turn.id,
                json!({"type":"question-answered","question":question}),
            )
            .unwrap();
        let mut late = question.clone();
        late.status = anchor_platform_session::QuestionStatus::Pending;
        late.answer = None;
        sessions
            .append_turn_event(
                "local",
                "pilot",
                &turn.id,
                json!({"type":"question","question":late}),
            )
            .unwrap();
        let mut foreign = question.clone();
        foreign.session = "other".into();
        sessions
            .append_turn_event(
                "local",
                "pilot",
                &turn.id,
                json!({"type":"question","question":foreign}),
            )
            .unwrap();
        sessions
            .finish_turn("local", "pilot", &turn.id, TurnStatus::Completed, None)
            .unwrap();
        let (second, _) = sessions
            .create_turn("local", "pilot", "second", Some("重复的输入"))
            .unwrap();
        let interrupted = sessions
            .create_question(
                "local",
                "pilot",
                &second.id,
                "还需要继续吗？",
                json!({"type":"object","properties":{}}),
            )
            .unwrap();
        sessions
            .finish_turn("local", "pilot", &second.id, TurnStatus::Stopped, None)
            .unwrap();
        let history = messages(&sessions, "local", "pilot").unwrap();
        let text = history
            .iter()
            .map(|message| {
                (
                    message["role"].as_str().unwrap(),
                    message["text"].as_str().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            text,
            vec![
                ("user", "重复的输入"),
                ("assistant", "回答前的说明"),
                ("assistant", "选择哪份报告？"),
                ("user", "确认：否\n报告：季度报告"),
                ("assistant", "回答后的回复"),
                ("user", "重复的输入"),
                ("assistant", "还需要继续吗？"),
            ]
        );
        assert_eq!(history[2]["question"], json!(question));
        assert_eq!(history[3]["question_answer"]["turn"], turn.id);
        assert_eq!(history[6]["question"]["id"], interrupted.id);
        assert_eq!(history[6]["question"]["status"], "interrupted");
        drop(sessions);
        let reopened = SessionStore::open(root.path().join("sessions.sqlite")).unwrap();
        assert_eq!(messages(&reopened, "local", "pilot").unwrap(), history);
        assert!(messages(&reopened, "other", "pilot").is_err());
    }

    #[test]
    fn declined_and_cancelled_answers_remain_explicit_user_messages() {
        let root = tempfile::tempdir().unwrap();
        let sessions = SessionStore::open(root.path().join("sessions.sqlite")).unwrap();
        sessions
            .create(
                "local",
                CreateSession {
                    id: Some("pilot".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let (turn, _) = sessions
            .create_turn("local", "pilot", "request", None)
            .unwrap();
        for action in [
            QuestionAction::Decline,
            QuestionAction::Cancel,
            QuestionAction::Accept,
        ] {
            let question = sessions
                .create_question(
                    "local",
                    "pilot",
                    &turn.id,
                    "是否继续？",
                    json!({"type":"object","properties":{}}),
                )
                .unwrap();
            sessions
                .answer_question(
                    "local",
                    "pilot",
                    &turn.id,
                    &question.id,
                    QuestionAnswer {
                        action,
                        content: (action == QuestionAction::Accept).then(|| json!({})),
                    },
                )
                .unwrap();
        }
        sessions
            .finish_turn("local", "pilot", &turn.id, TurnStatus::Completed, None)
            .unwrap();
        let history = messages(&sessions, "local", "pilot").unwrap();
        assert_eq!(history.len(), 6);
        assert_eq!(history[1]["text"], "已拒绝");
        assert_eq!(history[3]["text"], "已取消");
        assert_eq!(history[5]["text"], "已回答");
    }
}
