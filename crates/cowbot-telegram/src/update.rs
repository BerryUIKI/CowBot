//! Telegram's update shapes, and the two pieces of its text protocol that are
//! easy to get wrong (ADR-0058).
//!
//! Only what the adapter reads is modelled. `getUpdates` can also deliver
//! `callback_query`, `edited_message`, `poll_answer` and a dozen more; the poll
//! asks Telegram for `message` only, so the rest are not represented here.

use serde::Deserialize;

/// One delivered update.
#[derive(Debug, Clone, Deserialize)]
pub struct Update {
    /// Telegram's sequence number for this delivery.
    ///
    /// It is what confirms the poll: the next request sends `offset =
    /// update_id + 1`, and Telegram will not deliver anything below it again.
    /// It is also the dedupe key the pipeline claims, because a restart before
    /// the offset was confirmed replays the batch.
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<Message>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    pub message_id: i64,
    /// Unix seconds.
    #[serde(default)]
    pub date: i64,
    pub chat: Chat,
    /// Absent on a message posted anonymously to a channel, and on a few
    /// service messages.
    #[serde(default)]
    pub from: Option<User>,
    #[serde(default)]
    pub text: Option<String>,
    /// Where things like a mention or a command sit in `text`.
    #[serde(default)]
    pub entities: Vec<Entity>,
    /// The message this one answers.
    ///
    /// Telegram has no threads outside forum topics; quoting is how a person
    /// refers to something. It matters to the adapter only because replying to
    /// the bot is a way of addressing it.
    #[serde(default)]
    pub reply_to_message: Option<Box<Message>>,
    /// The forum topic this message was posted in.
    #[serde(default)]
    pub message_thread_id: Option<i64>,
    #[serde(default)]
    pub is_topic_message: Option<bool>,
}

impl Message {
    /// The forum topic this message belongs to, if any.
    ///
    /// Only a real topic counts. `reply_to_message` looks like a thread and is
    /// not one: answering into a made-up topic id would post into a forum topic
    /// that does not exist, or fail outright.
    pub fn topic(&self) -> Option<String> {
        match (self.is_topic_message, self.message_thread_id) {
            (Some(true), Some(id)) => Some(id.to_string()),
            _ => None,
        }
    }

    /// The `from` account, as a value rather than an `Option` to unwrap.
    pub fn author(&self) -> Option<&User> {
        self.from.as_ref()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Chat {
    pub id: i64,
    /// `private`, `group`, `supergroup`, or `channel`.
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
}

impl Chat {
    /// Whether this is a one-to-one conversation with the bot.
    ///
    /// A private chat needs no mention: every message in it is addressed to the
    /// bot, which is exactly CowBot's direct-conversation mode.
    pub fn is_private(&self) -> bool {
        self.kind == "private"
    }

    /// The name to show for this conversation.
    pub fn display_name(&self) -> String {
        if let Some(title) = self.title.clone().filter(|title| !title.is_empty()) {
            return title;
        }
        let parts: Vec<&str> = [self.first_name.as_deref(), self.last_name.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        if !parts.is_empty() {
            return parts.join(" ");
        }
        self.username
            .clone()
            .map(|name| format!("@{name}"))
            .unwrap_or_else(|| self.id.to_string())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: i64,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
}

impl User {
    /// The name to show, preferring the handle people recognise.
    pub fn display_name(&self) -> String {
        match self.username.as_deref() {
            Some(username) if !username.is_empty() => format!("@{username}"),
            // No handle: a first and last name is the only name there is.
            _ => {
                let last = self.last_name.as_deref().unwrap_or_default();
                let full = format!("{} {}", self.first_name, last).trim().to_string();
                if full.is_empty() {
                    self.id.to_string()
                } else {
                    full
                }
            }
        }
    }
}

/// One annotated span of a message's text.
#[derive(Debug, Clone, Deserialize)]
pub struct Entity {
    /// `mention`, `text_mention`, `bot_command`, `bold`, `code`, …
    #[serde(rename = "type")]
    pub kind: String,
    /// Offset in **UTF-16 code units**, not bytes and not characters.
    ///
    /// This is the single most likely way to get Telegram text handling wrong:
    /// an emoji before a mention is two units wide, so treating the offset as a
    /// character index slices the mention in the wrong place. [`slice_utf16`]
    /// is what converts it.
    pub offset: usize,
    pub length: usize,
    /// The user a `text_mention` points at.
    ///
    /// Present when a person was picked from a client's user list; it carries
    /// the account even when that account has no handle to type.
    #[serde(default)]
    pub user: Option<User>,
}

impl Entity {
    pub fn is_mention(&self) -> bool {
        self.kind == "mention"
    }

    pub fn is_text_mention(&self) -> bool {
        self.kind == "text_mention"
    }

    pub fn is_bot_command(&self) -> bool {
        self.kind == "bot_command"
    }
}

/// The span of `text` an entity names.
///
/// `None` when the offset falls inside a surrogate pair, which Telegram itself
/// cannot produce — but a hand-built payload in a test can, and panicking on a
/// malformed update would take the poll loop down.
pub fn slice_utf16(text: &str, offset: usize, length: usize) -> Option<&str> {
    let start = byte_index(text, offset)?;
    let end = byte_index(text, offset + length)?;
    Some(&text[start..end])
}

/// The byte index of a UTF-16 code-unit offset.
fn byte_index(text: &str, utf16: usize) -> Option<usize> {
    let mut units = 0usize;
    for (index, character) in text.char_indices() {
        if units == utf16 {
            return Some(index);
        }
        units += character.len_utf16();
        if units > utf16 {
            // The offset landed inside a surrogate pair.
            return None;
        }
    }
    (units == utf16).then_some(text.len())
}

/// Whether a message addresses the bot.
///
/// Three ways, and Telegram counts all three even with privacy mode on:
///
/// * a `mention` entity naming this bot's handle;
/// * a `text_mention` entity pointing at the bot's account — this is how a
///   client inserts a mention of an account that has no handle;
/// * a reply to one of the bot's own messages.
///
/// A `/command` counts too. `bot_command` entities cover the whole command, so
/// the handle has to be read out of the command: `/help@other_bot` addresses
/// that bot, not this one.
pub fn addresses_bot(text: &str, entities: &[Entity], reply_to_bot: bool, bot: &BotHandle) -> bool {
    if reply_to_bot {
        return true;
    }
    entities.iter().any(|entity| {
        if entity.is_mention() {
            slice_utf16(text, entity.offset, entity.length)
                .is_some_and(|mention| bot.matches_mention(mention))
        } else if entity.is_text_mention() {
            entity
                .user
                .as_ref()
                .is_some_and(|user| user.id.to_string() == bot.user_id)
        } else if entity.is_bot_command() {
            match slice_utf16(text, entity.offset, entity.length) {
                // `/help` with no suffix is for whichever bot receives it.
                Some(command) => match command.split_once('@') {
                    Some((_, handle)) => bot.matches_command_handle(handle),
                    None => true,
                },
                None => false,
            }
        } else {
            false
        }
    })
}

/// The bot's own identity, as the mention and command checks need it.
#[derive(Debug, Clone)]
pub struct BotHandle {
    pub user_id: String,
    pub username: String,
}

impl BotHandle {
    pub fn new(user_id: impl Into<String>, username: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            username: username.into(),
        }
    }

    /// Whether `candidate` — the text of a `mention` entity — names this bot.
    ///
    /// Telegram usernames are case-insensitive, and a client may normalise the
    /// case it sends, so the comparison is too.
    pub fn matches_mention(&self, candidate: &str) -> bool {
        let candidate = candidate.strip_prefix('@').unwrap_or(candidate);
        candidate.eq_ignore_ascii_case(&self.username)
    }

    /// Whether a command's `@handle` suffix names this bot.
    pub fn matches_command_handle(&self, candidate: &str) -> bool {
        candidate.eq_ignore_ascii_case(&self.username)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bot() -> BotHandle {
        BotHandle::new("42", "cowbot_help_bot")
    }

    fn entity(kind: &str, offset: usize, length: usize) -> Entity {
        Entity {
            kind: kind.into(),
            offset,
            length,
            user: None,
        }
    }

    fn message(json: serde_json::Value) -> Message {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn an_update_carries_its_sequence_number_and_message() {
        let update: Update = serde_json::from_str(
            r#"{"update_id":900719,"message":{"message_id":5,"date":1700000000,
                "chat":{"id":-1001234,"type":"supergroup","title":"Deploys"},
                "from":{"id":7,"is_bot":false,"first_name":"Ada","username":"ada"},
                "text":"hi"}}"#,
        )
        .unwrap();
        assert_eq!(update.update_id, 900719);
        let message = update.message.unwrap();
        assert_eq!(message.chat.id, -1001234);
        assert_eq!(message.chat.display_name(), "Deploys");
        assert_eq!(message.author().unwrap().display_name(), "@ada");
        assert!(!message.author().unwrap().is_bot);
    }

    #[test]
    fn a_private_chat_is_one_to_one_and_a_group_is_not() {
        assert!(
            message(serde_json::json!({"message_id":1,"chat":{"id":7,"type":"private"}}))
                .chat
                .is_private()
        );
        assert!(!message(
            serde_json::json!({"message_id":1,"chat":{"id":-7,"type":"group","title":"G"}})
        )
        .chat
        .is_private());
    }

    #[test]
    fn a_chat_with_no_title_falls_back_to_the_persons_name_then_the_id() {
        let person = message(
            serde_json::json!({"message_id":1,"chat":{"id":7,"type":"private",
            "first_name":"Ada","last_name":"Lovelace"}}),
        );
        assert_eq!(person.chat.display_name(), "Ada Lovelace");

        let handled = message(
            serde_json::json!({"message_id":1,"chat":{"id":7,"type":"private","username":"ada"}}),
        );
        assert_eq!(handled.chat.display_name(), "@ada");

        let anonymous =
            message(serde_json::json!({"message_id":1,"chat":{"id":7,"type":"private"}}));
        assert_eq!(anonymous.chat.display_name(), "7");
    }

    #[test]
    fn a_forum_topic_is_the_thread_and_a_quote_is_not() {
        let topic = message(
            serde_json::json!({"message_id":2,"chat":{"id":-7,"type":"supergroup"},
            "is_topic_message":true,"message_thread_id":88}),
        );
        assert_eq!(topic.topic().as_deref(), Some("88"));

        // A plain reply in a group carries `reply_to_message` and no topic id;
        // treating the quoted message as a thread would post into a topic that
        // does not exist.
        let quoted = message(
            serde_json::json!({"message_id":3,"chat":{"id":-7,"type":"group"},
            "reply_to_message":{"message_id":2,"chat":{"id":-7,"type":"group"}}}),
        );
        assert_eq!(quoted.topic(), None);
    }

    #[test]
    fn an_entity_offset_is_counted_in_utf16_code_units() {
        // "🚀 " is three UTF-16 units (two for the emoji, one for the space), so
        // a mention after it starts at 3, not at 2. Treating the offset as a
        // character index slices `@co` and reports the wrong text.
        let text = "🚀 @cowbot_help_bot go";
        let mention = entity("mention", 3, 16);
        assert_eq!(
            slice_utf16(text, mention.offset, mention.length),
            Some("@cowbot_help_bot")
        );
    }

    #[test]
    fn an_offset_inside_a_surrogate_pair_is_refused_rather_than_panicking() {
        assert_eq!(slice_utf16("🚀 hi", 1, 2), None);
        assert_eq!(slice_utf16("hi", 9, 1), None);
    }

    #[test]
    fn a_mention_of_the_bot_addresses_it_whatever_the_case() {
        let text = "@CowBot_Help_Bot status?";
        assert!(addresses_bot(
            text,
            &[entity("mention", 0, 16)],
            false,
            &bot()
        ));
        // A different bot's handle does not.
        let other = "@other_bot status?";
        assert!(!addresses_bot(
            other,
            &[entity("mention", 0, 10)],
            false,
            &bot()
        ));
        // Nor does an address, which contains the same characters.
        let email = "mail ada@cowbot_help_bot.example";
        assert!(!addresses_bot(email, &[], false, &bot()));
    }

    #[test]
    fn a_text_mention_addresses_the_bot_by_account_not_by_name() {
        let text = "Ada Lovelace can you check?";
        let mut named = entity("text_mention", 0, 12);
        named.user = Some(User {
            id: 42,
            is_bot: true,
            first_name: "CowBot".into(),
            last_name: None,
            username: None,
        });
        assert!(addresses_bot(text, &[named.clone()], false, &bot()));

        // The same span pointing at somebody else is not an address.
        named.user.as_mut().unwrap().id = 43;
        assert!(!addresses_bot(text, &[named], false, &bot()));
    }

    #[test]
    fn a_reply_to_the_bot_addresses_it() {
        assert!(addresses_bot("thanks", &[], true, &bot()));
        assert!(!addresses_bot("thanks", &[], false, &bot()));
    }

    #[test]
    fn a_command_addresses_the_bot_unless_it_names_another_one() {
        assert!(addresses_bot(
            "/help",
            &[entity("bot_command", 0, 5)],
            false,
            &bot()
        ));
        assert!(addresses_bot(
            "/help@cowbot_help_bot",
            &[entity("bot_command", 0, 21)],
            false,
            &bot()
        ));
        // `/help@other_bot` is that bot's command; answering it would be wrong.
        assert!(!addresses_bot(
            "/help@other_bot",
            &[entity("bot_command", 0, 15)],
            false,
            &bot()
        ));
    }

    #[test]
    fn bold_text_and_a_command_without_a_command_entity_do_not_address_the_bot() {
        assert!(!addresses_bot(
            "**hello**",
            &[entity("bold", 0, 9)],
            false,
            &bot()
        ));
        // A URL with an @ in it is a `url` entity, not a mention.
        assert!(!addresses_bot(
            "https://t.me/join/@cowbot_help_bot",
            &[entity("url", 0, 34)],
            false,
            &bot()
        ));
    }
}
