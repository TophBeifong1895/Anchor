use futures::SinkExt;
use serde_json::json;
use tokio::time::Instant;

use super::status;
use super::{AwaitingAck, Driver, MAX_IN_FLIGHT, connection::Socket};
use crate::{
    GatewayError,
    config::allowed,
    protocol::{ConversationScope, frame},
    stream_id,
};

pub(crate) const PROCESSING_TEXT: &str = "正在处理";
pub(crate) const FAILURE_TEXT: &str =
    "本次处理未能完成，请稍后补充一条消息继续；已执行的操作不会自动撤销。";
pub(crate) const ASSISTANT_STOPPED_TEXT: &str =
    "助手已停止。请打开 Anchor 的渠道页面，恢复该助手后再发消息。";
pub(crate) const SUPERSEDED_TEXT: &str = "已被你的新消息接续处理。";
pub(crate) const GROUP_SUPERSEDED_TEXT: &str = "已由群内的新消息接续处理。";
/// Progress lines update one bubble in place: at most this many per turn and at
/// most one per interval, so a chatty turn cannot flood the platform.
pub(super) const MAX_PROGRESS_UPDATES: usize = 15;
const PROGRESS_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

impl Driver {
    /// Keep at most one open progress bubble per conversation, and close the
    /// ones a newer message replaced.
    ///
    /// The platform binds a reply to the callback it answers, so a superseded
    /// message cannot be finished by the newer message's reply. Instead: a
    /// message that is superseded before its acknowledgement is due never shows
    /// a bubble at all, and one that already showed a bubble is closed
    /// explicitly, so no bubble is left saying "processing" forever.
    pub(super) async fn dispatch_progress(
        &mut self,
        socket: &mut Socket,
    ) -> Result<bool, GatewayError> {
        if !self.dispatch_feedback(socket).await? {
            return Ok(false);
        }
        if !self.close_superseded(socket).await? {
            return Ok(false);
        }
        if !self.show_due_acknowledgements(socket).await? {
            return Ok(false);
        }
        self.flush_progress(socket).await
    }

    /// Send changed status lines. The platform renders its own loading indicator,
    /// so unchanged bubbles need no periodic updates.
    ///
    /// A line that arrives inside the throttle window waits instead of being
    /// dropped, so the bubble still ends up current.
    async fn flush_progress(&mut self, socket: &mut Socket) -> Result<bool, GatewayError> {
        let now = Instant::now();
        let mut due = Vec::new();
        for (scope, bubble) in &self.open_bubbles {
            if !allowed(&self.config.inbound_users, &bubble.sender_id)
                || self.feedback_pending.contains_key(&bubble.request_id)
                || bubble.updates >= MAX_PROGRESS_UPDATES
                || now < bubble.next_update
            {
                continue;
            }
            let Some(content) = &bubble.pending else {
                continue;
            };
            due.push((
                scope.clone(),
                bubble.request_id.clone(),
                bubble.stream_id.clone(),
                content.clone(),
            ));
        }
        for (key, request_id, stream, content) in due {
            let value = frame(
                "aibot_respond_msg",
                &request_id,
                json!({
                    "msgtype":"stream",
                    "stream":{"id":stream,"content":content,"finish":false}
                }),
            );
            if !matches!(
                tokio::time::timeout(
                    self.config.timing.ack_timeout,
                    socket.send(tungstenite_text(value))
                )
                .await,
                Ok(Ok(()))
            ) {
                self.report(GatewayError::Unconfirmed);
                return Ok(false);
            }
            if let Some(bubble) = self.open_bubbles.get_mut(&key) {
                bubble.pending = None;
                bubble.content = content;
                bubble.updates += 1;
                bubble.next_update = now + PROGRESS_MIN_INTERVAL;
            }
            // Acknowledgements are keyed by request id, so at most one update
            // per bubble may be outstanding.
            self.feedback_pending.insert(
                request_id.clone(),
                AwaitingAck {
                    deadline: now + self.config.timing.ack_timeout,
                },
            );
            eprintln!("anchor-wecom-gateway: stream update {request_id} finish=false status");
        }
        Ok(true)
    }

    /// A message the user superseded stops waiting: its bubble is finished with
    /// one short, honest line instead of hanging.
    async fn close_superseded(&mut self, socket: &mut Socket) -> Result<bool, GatewayError> {
        let mut superseded = Vec::new();
        for (scope, bubble) in &self.open_bubbles {
            if !allowed(&self.config.inbound_users, &bubble.sender_id)
                || !self.ledger.is_latest_event(&bubble.event_id)?
            {
                superseded.push((
                    scope.clone(),
                    bubble.event_id.clone(),
                    bubble.request_id.clone(),
                    bubble.stream_id.clone(),
                    allowed(&self.config.inbound_users, &bubble.sender_id),
                ));
            }
        }
        // A turn the Host is not running any more has nothing left to report:
        // the bubble is closing, so its progress stream closes with it.
        let closed: Vec<String> = superseded
            .iter()
            .map(|(_, event_id, _, _, _)| event_id.clone())
            .collect();
        for event_id in closed {
            self.cancel_progress(&event_id);
        }
        for (key, _, request_id, stream, actor_allowed) in superseded {
            if !actor_allowed {
                self.open_bubbles.remove(&key);
                continue;
            }
            if self.feedback_pending.contains_key(&request_id) {
                continue;
            }
            let value = frame(
                "aibot_respond_msg",
                &request_id,
                json!({
                    "msgtype":"stream",
                    "stream":{"id":stream, "content":if matches!(key, ConversationScope::Group { .. }) {
                        GROUP_SUPERSEDED_TEXT
                    } else {
                        SUPERSEDED_TEXT
                    }, "finish":true}
                }),
            );
            if !matches!(
                tokio::time::timeout(
                    self.config.timing.ack_timeout,
                    socket.send(tungstenite_text(value))
                )
                .await,
                Ok(Ok(()))
            ) {
                self.report(GatewayError::Unconfirmed);
                return Ok(false);
            }
            eprintln!("anchor-wecom-gateway: stream update {request_id} finish=true superseded");
            self.open_bubbles.remove(&key);
            self.feedback_pending.insert(
                request_id,
                AwaitingAck {
                    deadline: Instant::now() + self.config.timing.ack_timeout,
                },
            );
        }
        Ok(true)
    }

    /// Show the processing bubble for messages that are still the newest and
    /// still being worked on after the acknowledgement delay.
    async fn show_due_acknowledgements(
        &mut self,
        socket: &mut Socket,
    ) -> Result<bool, GatewayError> {
        let now = Instant::now();
        let due: Vec<usize> = self
            .acks
            .iter()
            .enumerate()
            .filter(|(_, ack)| ack.deadline <= now)
            .map(|(index, _)| index)
            .collect();
        for index in due.into_iter().rev() {
            let ack = self.acks.remove(index);
            if self.feedback_pending.len() + self.pending.len() >= MAX_IN_FLIGHT {
                continue;
            }
            let key = ack.event.conversation_scope();
            if self.open_bubbles.contains_key(&key)
                || !allowed(&self.config.inbound_users, &ack.event.sender_id)
                || !self.ledger.is_latest(&ack.event)?
                || !self.ledger.is_processing(&ack.event.event_id)?
            {
                continue;
            }
            let Some(request_id) = ack.event.metadata["request_id"].as_str() else {
                return Err(GatewayError::Ledger);
            };
            // The placeholder is a status line too, so it goes through the same
            // rendering and carries the default category icon.
            let content = ack
                .line
                .clone()
                .unwrap_or_else(|| status::render(Some(status::DEFAULT), None, PROCESSING_TEXT));
            let value = frame(
                "aibot_respond_msg",
                request_id,
                json!({
                    "msgtype":"stream",
                    "stream":{
                        "id":stream_id(&ack.event.event_id),
                        "content":content,
                        "finish":false
                    }
                }),
            );
            if !matches!(
                tokio::time::timeout(
                    self.config.timing.ack_timeout,
                    socket.send(tungstenite_text(value))
                )
                .await,
                Ok(Ok(()))
            ) {
                self.report(GatewayError::Unconfirmed);
                return Ok(false);
            }
            eprintln!("anchor-wecom-gateway: stream update {request_id} finish=false progress");
            self.open_bubbles.insert(
                key,
                super::OpenBubble {
                    event_id: ack.event.event_id.clone(),
                    sender_id: ack.event.sender_id.clone(),
                    request_id: request_id.to_owned(),
                    stream_id: stream_id(&ack.event.event_id),
                    pending: None,
                    content: content.clone(),
                    next_update: Instant::now() + PROGRESS_MIN_INTERVAL,
                    updates: 0,
                },
            );
            self.feedback_pending.insert(
                request_id.to_owned(),
                AwaitingAck {
                    deadline: Instant::now() + self.config.timing.ack_timeout,
                },
            );
        }
        Ok(true)
    }

    async fn dispatch_feedback(&mut self, socket: &mut Socket) -> Result<bool, GatewayError> {
        for _ in 0..self.feedback.len() {
            if self.pending.len() + self.feedback_pending.len() >= MAX_IN_FLIGHT {
                break;
            }
            let Some(feedback) = self.feedback.pop_front() else {
                break;
            };
            if !allowed(&self.config.inbound_users, &feedback.event.sender_id)
                || !self.ledger.is_latest(&feedback.event)?
            {
                continue;
            }
            let Some(request_id) = feedback.event.metadata["request_id"].as_str() else {
                return Err(GatewayError::Ledger);
            };
            if self.pending.contains_key(request_id)
                || self.feedback_pending.contains_key(request_id)
            {
                self.feedback.push_back(feedback);
                continue;
            }
            let value = frame(
                "aibot_respond_msg",
                request_id,
                json!({
                    "msgtype":"stream",
                    "stream":{
                        "id":stream_id(&feedback.event.event_id),
                        "content":if feedback.assistant_stopped {
                            ASSISTANT_STOPPED_TEXT
                        } else if feedback.finish {
                            FAILURE_TEXT
                        } else {
                            PROCESSING_TEXT
                        },
                        "finish":feedback.finish
                    }
                }),
            );
            eprintln!(
                "anchor-wecom-gateway: stream update {request_id} finish={} notice",
                feedback.finish
            );
            self.feedback_pending.insert(
                request_id.to_owned(),
                AwaitingAck {
                    deadline: Instant::now() + self.config.timing.ack_timeout,
                },
            );
            if !matches!(
                tokio::time::timeout(
                    self.config.timing.ack_timeout,
                    socket.send(tungstenite_text(value))
                )
                .await,
                Ok(Ok(()))
            ) {
                self.report(GatewayError::Unconfirmed);
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn tungstenite_text(value: serde_json::Value) -> tokio_tungstenite::tungstenite::Message {
    tokio_tungstenite::tungstenite::Message::Text(value.to_string().into())
}
