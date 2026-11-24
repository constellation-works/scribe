//! Fail-closed adapter for Codex desktop rollout messages. This is an
//! intentionally version-sensitive interface, not a stable Codex contract.
//! Host content metadata, rather than a role or hook payload claim, identifies
//! typed user text. Same-user file forgery remains outside this accident guard.

use super::Message;
use crate::{SessionId, Speaker};
use serde_json::Value;
use std::collections::BTreeSet;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// One host-attributed human input, separate from injected skill instructions.
#[derive(Clone, Debug)]
pub struct TypedTurn {
    /// Stable host turn id (several input messages may share it).
    pub turn_id: String,
    /// The message id, timestamp and user-owned text blocks.
    pub message: Message,
}

/// Verified main-thread transcript metadata and supported conversation items.
#[derive(Debug)]
pub struct Rollout {
    /// Session id in the host's session metadata.
    pub session: SessionId,
    /// Working directory recorded by the host.
    pub cwd: String,
    /// User-owned input messages, in file order.
    pub typed: Vec<TypedTurn>,
    /// Typed inputs and assistant text; never tools, system or skills.
    pub messages: Vec<Message>,
}

/// Parse a supported main user thread. Unsupported or contradictory identity
/// metadata is refused, never guessed from message roles.
pub fn parse(bytes: &[u8]) -> Result<Rollout, String> {
    let mut rows = Vec::new();
    for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let value = serde_json::from_slice::<Value>(line)
            .map_err(|_| String::from("unsupported Codex transcript: malformed JSONL; retry after the host finishes writing"))?;
        rows.push(value);
    }
    let metas: Vec<_> = rows
        .iter()
        .filter(|r| r["type"] == "session_meta")
        .collect();
    if metas.len() != 1 {
        return Err(String::from(
            "unsupported Codex transcript: expected one session_meta",
        ));
    }
    let meta = &metas[0]["payload"];
    let source = meta["source"].as_str();
    if !matches!(source, Some("vscode" | "cli")) || meta["thread_source"] != "user" {
        return Err(String::from(
            "Codex transcript is not a supported main user thread (subagent, automation or missing thread_source)",
        ));
    }
    let session: SessionId = meta["id"]
        .as_str()
        .ok_or("Codex session_meta has no id")?
        .parse()
        .map_err(|_| "invalid Codex session id")?;
    if meta
        .get("session_id")
        .is_some_and(|id| id.as_str() != Some(session.as_str()))
    {
        return Err(String::from("contradictory Codex session identity"));
    }
    let cwd = meta["cwd"]
        .as_str()
        .ok_or("Codex session_meta has no cwd")?
        .to_owned();
    let mut messages = Vec::new();
    let mut typed = Vec::new();
    let mut ids = BTreeSet::new();
    for row in &rows {
        if row["type"] != "response_item" {
            continue;
        }
        let item = &row["payload"];
        if item["type"] != "message" {
            continue;
        }
        let Some(content) = item["content"].as_array() else {
            continue;
        };
        let at = row["timestamp"]
            .as_str()
            .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
            .ok_or("Codex message has no valid timestamp")?;
        let role = match item["role"].as_str() {
            Some("user") => Speaker::User,
            Some("assistant") => Speaker::Assistant,
            _ => continue,
        };
        let mut turn = None;
        let blocks: Vec<String> = if role == Speaker::User {
            let metadata = &item["internal_chat_message_metadata_passthrough"];
            let (Some(kinds), Some(origins), Some(turn_id)) = (
                metadata["content_item_kinds"].as_array(),
                metadata["content_item_metadata"].as_array(),
                metadata["turn_id"].as_str().filter(|s| !s.is_empty()),
            ) else {
                continue;
            };
            if kinds.len() != content.len() || origins.len() != content.len() {
                continue;
            }
            turn = Some(turn_id.to_owned());
            content
                .iter()
                .zip(kinds)
                .zip(origins)
                .filter_map(|((block, kind), origin)| {
                    (kind == "user.text"
                        && origin["provenance"]["type"] == "user"
                        && block["type"] == "input_text")
                        .then(|| block["text"].as_str().map(str::to_owned))
                        .flatten()
                })
                .collect()
        } else {
            content
                .iter()
                .filter(|b| b["type"] == "output_text")
                .filter_map(|b| b["text"].as_str().map(str::to_owned))
                .collect()
        };
        if blocks.is_empty() {
            continue;
        }
        let uuid = item["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "codex-{}",
                    scribe_store::digest::prompt_sha256(&row.to_string())
                )
            });
        if !ids.insert(uuid.clone()) {
            return Err(String::from("duplicate Codex message identity"));
        }
        let message = Message {
            uuid,
            role,
            at,
            blocks,
        };
        if let Some(turn_id) = turn {
            typed.push(TypedTurn {
                turn_id,
                message: message.clone(),
            });
        }
        messages.push(message);
    }
    Ok(Rollout {
        session,
        cwd,
        typed,
        messages,
    })
}
