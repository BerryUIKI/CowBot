//! Feishu's event and callback shapes, and the two pieces of its text protocol
//! that are easy to get wrong (ADR-0061).
//!
//! Three rules here are the ones that bite:
//!
//! * **The dedupe key is the message id, not the event id.** Feishu's own
//!   documentation says repeated pushes can occur and that idempotency must use
//!   `message_id`. This is the opposite of every other platform in this
//!   repository, and getting it wrong produces two turns and two charges for one
//!   question.
//! * **`content` is a JSON string inside JSON.** The text does not exist until it
//!   has been parsed a second time.
//! * **A mention is a placeholder.** The text carries `@_user_1` and the name is
//!   only in `mentions[]`, joined by `key`. Stored unchanged, every message reads
//!   as machine tokens and no agent is ever addressed.

use serde::Deserialize;

/// The envelope an event arrives in.
///
/// `schema: "2.0"` is the only version modelled. A v1 event puts its fields at
/// the top level and has no `schema`, and the adapter refuses it rather than
/// guessing.
#[derive(Debug, Clone, Deserialize)]
pub struct Envelope {
    #[serde(default)]
    pub schema: String,
    pub header: Header,
    #[serde(default)]
    pub event: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Header {
    /// Unique per delivery — and, per Feishu's own documentation, not what a
    /// consumer should deduplicate on for a message.
    #[serde(default)]
    pub event_id: String,
    #[serde(default)]
    pub event_type: String,
    /// Milliseconds since the epoch, as a string.
    #[serde(default)]
    pub create_time: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub tenant_key: String,
}

impl Envelope {
    /// Whether this is the v2 envelope this adapter understands.
    ///
    /// A v1 event would be read as an event with every field empty, which is a
    /// message dropped with no explanation.
    pub fn is_v2(&self) -> bool {
        self.schema == "2.0"
    }

    /// Whether the event came from the app this adapter is configured for.
    ///
    /// The long connection authenticates the app, so this is a second check
    /// rather than the first; it costs one string compare and stops an app id
    /// pasted into the wrong integration from answering.
    pub fn is_for(&self, app_id: &str) -> bool {
        self.header.app_id == app_id
    }

    pub fn event_type(&self) -> &str {
        &self.header.event_type
    }
}

/// A `im.message.receive_v1` event.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageEvent {
    pub sender: Sender,
    pub message: Message,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Sender {
    #[serde(default)]
    pub sender_id: SenderId,
    /// `user` for a person. Feishu also reports `bot` and `app`, and both mean
    /// "not a person": answering either is how an echo loop starts.
    #[serde(default)]
    pub sender_type: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SenderId {
    /// The only id always present. `user_id` needs an extra scope and
    /// `union_id` is developer-scoped, so keying a principal on either gives
    /// every person a different answer depending on the tenant's permissions.
    #[serde(default)]
    pub open_id: String,
    #[serde(default)]
    pub union_id: String,
    #[serde(default)]
    pub user_id: Option<String>,
}

impl Sender {
    /// Whether a person sent this.
    ///
    /// Only `user` is a person. `bot` and `app` are both this or another bot.
    pub fn is_person(&self) -> bool {
        self.sender_type == "user"
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub message_id: String,
    #[serde(default)]
    pub root_id: String,
    #[serde(default)]
    pub parent_id: String,
    /// Milliseconds since the epoch, as a string.
    #[serde(default)]
    pub create_time: String,
    #[serde(default)]
    pub chat_id: String,
    /// Present only on a topic message; absent, not null, on an ordinary group
    /// message — which is why it is defaulted.
    #[serde(default)]
    pub thread_id: Option<String>,
    /// `group` or `p2p`.
    #[serde(default)]
    pub chat_type: String,
    /// `text`, `post`, `image`, `file`, `audio`, `media`, and others.
    #[serde(default)]
    pub message_type: String,
    /// A JSON **string** that has to be parsed a second time.
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub mentions: Vec<Mention>,
}

/// One mention, which resolves a placeholder in the text.
#[derive(Debug, Clone, Deserialize)]
pub struct Mention {
    /// The literal placeholder the text carries, `@_user_1`.
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub id: SenderId,
    /// `user` or `bot`.
    #[serde(default)]
    pub mentioned_type: String,
    #[serde(default)]
    pub name: String,
}

impl Message {
    /// Whether this is a one-to-one conversation with the bot.
    ///
    /// `p2p` is CowBot's direct-conversation mode: every message is addressed, so
    /// no mention is needed (ADR-0024). Treating it as a group makes the bot
    /// silent in direct messages, which is the failure this method prevents.
    pub fn is_direct(&self) -> bool {
        self.chat_type == "p2p"
    }

    /// The thread this message belongs to, if it is a topic message.
    pub fn topic(&self) -> Option<String> {
        self.thread_id.clone().filter(|id| !id.is_empty())
    }

    /// The message's text, with every mention replaced by the name Feishu gave.
    ///
    /// Returns `None` for a message type whose content is not text — an image or
    /// a file — because reading `<image_key>` as a question is worse than storing
    /// nothing.
    pub fn text(&self) -> Option<String> {
        let raw: serde_json::Value = serde_json::from_str(&self.content).ok()?;
        let text = match self.message_type.as_str() {
            "text" => raw.get("text")?.as_str()?.to_string(),
            // A rich-text post is a nested structure keyed by locale. The plain
            // `text` block is what a person typed, and joining the runs keeps a
            // sentence in one piece rather than one line per run.
            "post" => post_text(&raw)?,
            _ => return None,
        };
        Some(resolve_mentions(text, &self.mentions))
    }

    /// What the message carried, for the pipeline's attachment handling.
    pub fn attachment(&self) -> Option<cowbot_integrations::model::Attachment> {
        use cowbot_integrations::model::{Attachment, AttachmentKind};
        // The key lives in a JSON string, the same as the text does.
        let raw: serde_json::Value = serde_json::from_str(&self.content).ok()?;
        let (key, kind) = match self.message_type.as_str() {
            "image" => (
                raw.get("image_key")?.as_str()?.to_string(),
                AttachmentKind::Image,
            ),
            "file" => (
                raw.get("file_key")?.as_str()?.to_string(),
                AttachmentKind::File,
            ),
            "audio" => (
                raw.get("file_key")?.as_str()?.to_string(),
                AttachmentKind::Audio,
            ),
            "media" => (
                raw.get("file_key")?.as_str()?.to_string(),
                AttachmentKind::Video,
            ),
            _ => return None,
        };
        Some(Attachment {
            external_id: key,
            kind,
            // Feishu does not report a name on the message, and the file's own
            // metadata lives behind a separate call the pipeline does not make.
            file_name: raw
                .get("file_name")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            mime_type: None,
            size_bytes: None,
        })
    }

    /// A moment from the message's own timestamp.
    pub fn moment(&self) -> chrono::DateTime<chrono::Utc> {
        millis(&self.create_time)
    }
}

/// The text of a rich-text post.
fn post_text(raw: &serde_json::Value) -> Option<String> {
    // `{"zh_cn":{"title":"…","content":[[{tag,text},…],…]}}`, or the same under
    // `en_us`. Either is usable; the title is included when it is there.
    let locale = raw
        .get("zh_cn")
        .or_else(|| raw.get("en_us"))
        .or_else(|| raw.as_object()?.values().next())?;
    let mut parts = Vec::new();
    if let Some(title) = locale.get("title").and_then(|value| value.as_str()) {
        if !title.trim().is_empty() {
            parts.push(title.to_string());
        }
    }
    for paragraph in locale.get("content")?.as_array()? {
        let mut line = String::new();
        for run in paragraph.as_array().into_iter().flatten() {
            if let Some(text) = run.get("text").and_then(|value| value.as_str()) {
                line.push_str(text);
            }
        }
        if !line.trim().is_empty() {
            parts.push(line);
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// Replace every `@_user_N` placeholder with the name Feishu reported for it.
///
/// `@_all` is the broadcast mention and is handled first: a key-to-name loop that
/// treated it like any other would produce `@` with no name, and `parse_mentions`
/// reads that as an unknown agent and posts a notice nobody asked for.
pub fn resolve_mentions(text: String, mentions: &[Mention]) -> String {
    let mut text = text;
    for mention in mentions {
        if mention.key.is_empty() {
            continue;
        }
        let replacement = if mention.key == "@_all" || mention.key == "@all" {
            "@all".to_string()
        } else if mention.name.trim().is_empty() {
            // A mention with no name is left as the placeholder: an id reads as
            // an id, while an invented name would address the wrong reader.
            continue;
        } else {
            format!("@{}", mention.name.trim())
        };
        text = text.replace(&mention.key, &replacement);
    }
    text
}

/// Whether the bot was mentioned, by its own `open_id`.
///
/// Read from `mentions[]` rather than the text, because the text only carries a
/// placeholder that resolves to a name, and a name is not unique.
pub fn mentions_bot(mentions: &[Mention], bot_open_id: &str) -> bool {
    mentions
        .iter()
        .any(|mention| mention.id.open_id == bot_open_id && !bot_open_id.is_empty())
}

/// A millisecond timestamp carried as a string.
///
/// Feishu sends the event timestamp in milliseconds and the **card callback**
/// timestamp in microseconds, so a caller must not reuse one for the other. An
/// unparsable value falls back to now: a message dated 1970 sorts to the top of
/// every history query.
pub fn millis(value: &str) -> chrono::DateTime<chrono::Utc> {
    value
        .parse::<i64>()
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .unwrap_or_else(chrono::Utc::now)
}

/// A `card.action.trigger` callback.
#[derive(Debug, Clone, Deserialize)]
pub struct CardAction {
    #[serde(default)]
    pub operator: Operator,
    /// The 30-minute card update token. Usable twice, and only **after** the
    /// callback has been answered.
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub action: Action,
    #[serde(default)]
    pub context: ActionContext,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Operator {
    #[serde(default)]
    pub open_id: String,
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Action {
    /// Whatever the button's `behaviors[].value` carried: an object or a string.
    /// A deserializer that demanded an object would fail on a correctly built
    /// button.
    #[serde(default)]
    pub value: serde_json::Value,
    #[serde(default)]
    pub tag: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ActionContext {
    #[serde(default)]
    pub open_message_id: String,
    #[serde(default)]
    pub open_chat_id: String,
}

impl Action {
    /// The approval id and answer this button carried.
    ///
    /// The value is `{"application": "approve", "approval_id": "…"}` when it was
    /// built as an object, and `approve:…` when it was built as a string. Both
    /// are read, because both are shapes Feishu accepts.
    pub fn decision(&self) -> Option<(bool, String)> {
        let (answer, approval_id) = match &self.value {
            serde_json::Value::String(value) => {
                let (answer, approval_id) = value.split_once(':')?;
                (answer.to_string(), approval_id.to_string())
            }
            serde_json::Value::Object(map) => {
                let answer = map.get("application")?.as_str()?.to_string();
                let approval_id = map.get("approval_id")?.as_str()?.to_string();
                (answer, approval_id)
            }
            _ => return None,
        };
        let approved = match answer.as_str() {
            "approve" => true,
            "reject" => false,
            _ => return None,
        };
        (!approval_id.is_empty()).then_some((approved, approval_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(json: serde_json::Value) -> Message {
        serde_json::from_value(json).unwrap()
    }

    fn mention(key: &str, name: &str, open_id: &str) -> Mention {
        Mention {
            key: key.into(),
            id: SenderId {
                open_id: open_id.into(),
                ..SenderId::default()
            },
            mentioned_type: "user".into(),
            name: name.into(),
        }
    }

    #[test]
    fn the_content_field_is_json_inside_json_and_must_be_parsed_twice() {
        // Storing it unparsed shows the agent the literal `{"text":"hi"}`.
        let message = message(serde_json::json!({
            "message_id": "om_1", "chat_id": "oc_1", "chat_type": "group",
            "message_type": "text", "content": "{\"text\":\"hi there\"}",
        }));
        assert_eq!(message.text().as_deref(), Some("hi there"));
    }

    #[test]
    fn a_mention_placeholder_becomes_the_name_feishu_reported() {
        let message = message(serde_json::json!({
            "message_id": "om_1", "chat_id": "oc_1", "chat_type": "group",
            "message_type": "text", "content": "{\"text\":\"@_user_1 status?\"}",
            "mentions": [{ "key": "@_user_1", "name": "CowBot",
                           "id": { "open_id": "ou_bot" }, "mentioned_type": "bot" }],
        }));
        // A machine placeholder in the timeline means no agent is addressed.
        assert_eq!(message.text().as_deref(), Some("@CowBot status?"));
    }

    #[test]
    fn a_broadcast_mention_does_not_become_a_bare_at_sign() {
        let text = resolve_mentions("@_all standup".into(), &[mention("@_all", "", "ou_all")]);
        assert_eq!(text, "@all standup");
    }

    #[test]
    fn a_mention_with_no_name_keeps_its_placeholder_rather_than_inventing_one() {
        let text = resolve_mentions("@_user_1 hi".into(), &[mention("@_user_1", "  ", "ou_1")]);
        assert_eq!(text, "@_user_1 hi");
    }

    #[test]
    fn the_bot_is_recognised_by_its_open_id_and_not_by_its_name() {
        let mentions = vec![mention("@_user_1", "CowBot", "ou_bot")];
        assert!(mentions_bot(&mentions, "ou_bot"));
        assert!(!mentions_bot(&mentions, "ou_other"));
        // An empty id must not match every mention.
        assert!(!mentions_bot(&mentions, ""));
    }

    #[test]
    fn p2p_is_direct_and_group_is_not() {
        let direct = message(serde_json::json!({
            "message_id": "om_1", "chat_type": "p2p", "message_type": "text",
            "content": "{\"text\":\"hi\"}",
        }));
        assert!(direct.is_direct());

        let group = message(serde_json::json!({
            "message_id": "om_1", "chat_type": "group", "message_type": "text",
            "content": "{\"text\":\"hi\"}",
        }));
        assert!(!group.is_direct());
    }

    #[test]
    fn a_topic_message_has_a_thread_and_an_ordinary_one_has_no_key_at_all() {
        let topic = message(serde_json::json!({
            "message_id": "om_1", "chat_type": "group", "message_type": "text",
            "content": "{\"text\":\"hi\"}", "thread_id": "omt_1",
        }));
        assert_eq!(topic.topic().as_deref(), Some("omt_1"));

        // Absent, not null: a deserializer that required the key would fail on
        // the common case.
        let plain = message(serde_json::json!({
            "message_id": "om_1", "chat_type": "group", "message_type": "text",
            "content": "{\"text\":\"hi\"}",
        }));
        assert_eq!(plain.topic(), None);
    }

    #[test]
    fn only_a_user_sender_is_a_person() {
        // `app` is what Feishu reports for a bot through the send API, and
        // treating it as a person is an echo loop.
        for kind in ["bot", "app", ""] {
            let sender: Sender = serde_json::from_value(serde_json::json!({
                "sender_type": kind, "sender_id": { "open_id": "ou_1" }
            }))
            .unwrap();
            assert!(!sender.is_person(), "{kind}");
        }
        let person: Sender = serde_json::from_value(serde_json::json!({
            "sender_type": "user", "sender_id": { "open_id": "ou_1" }
        }))
        .unwrap();
        assert!(person.is_person());
    }

    #[test]
    fn an_image_message_yields_an_attachment_and_no_text() {
        let image = message(serde_json::json!({
            "message_id": "om_1", "chat_type": "group", "message_type": "image",
            "content": "{\"image_key\":\"img_v2_1\"}",
        }));
        assert_eq!(image.text(), None);
        let attachment = image.attachment().expect("an attachment");
        assert_eq!(attachment.external_id, "img_v2_1");
        assert_eq!(
            attachment.kind,
            cowbot_integrations::model::AttachmentKind::Image
        );
    }

    #[test]
    fn a_rich_text_post_is_read_as_its_paragraphs() {
        let post = message(serde_json::json!({
            "message_id": "om_1", "chat_type": "group", "message_type": "post",
            "content": "{\"zh_cn\":{\"title\":\"Deploy\",\"content\":[[{\"tag\":\"text\",\"text\":\"is \"},{\"tag\":\"text\",\"text\":\"green\"}]]}}",
        }));
        assert_eq!(post.text().as_deref(), Some("Deploy\nis green"));
    }

    #[test]
    fn a_button_value_is_read_from_either_shape_feishu_accepts() {
        let object: Action = serde_json::from_value(serde_json::json!({
            "value": { "application": "approve", "approval_id": "a1" }
        }))
        .unwrap();
        assert_eq!(object.decision(), Some((true, "a1".into())));

        // A correctly built button may send a plain string; demanding an object
        // would fail on it.
        let string: Action = serde_json::from_value(serde_json::json!({
            "value": "reject:a1"
        }))
        .unwrap();
        assert_eq!(string.decision(), Some((false, "a1".into())));

        let foreign: Action = serde_json::from_value(serde_json::json!({ "value": "x" })).unwrap();
        assert_eq!(foreign.decision(), None);
    }

    #[test]
    fn a_v1_envelope_is_refused_rather_than_read_as_an_empty_event() {
        let v1: Envelope = serde_json::from_value(serde_json::json!({
            "type": "event_callback",
            "header": { "event_type": "im.message.receive_v1" },
        }))
        .unwrap();
        assert!(!v1.is_v2(), "a v1 event has no schema");

        let v2: Envelope = serde_json::from_value(serde_json::json!({
            "schema": "2.0",
            "header": { "event_type": "im.message.receive_v1", "app_id": "cli_1" },
        }))
        .unwrap();
        assert!(v2.is_v2());
        assert!(v2.is_for("cli_1"));
        assert!(!v2.is_for("cli_2"));
    }

    #[test]
    fn an_unparsable_timestamp_falls_back_to_now_rather_than_1970() {
        assert!(millis("nonsense").timestamp() > 1_600_000_000);
        assert_eq!(
            millis("1700000000000").timestamp(),
            1_700_000_000,
            "milliseconds, not seconds"
        );
    }
}
