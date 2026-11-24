//! The outbound mirror: a channel message back into the IM (ADR-0056).
//!
//! The pipeline subscribes to the workspace bus and hands every `MessagePosted`
//! whose channel is bound to the adapter. Two filter rules matter:
//!
//! * only `Sender::Agent` is mirrored — the IM already shows what its own users
//!   typed, and echoing it would duplicate every message;
//! * a reply goes back to the external conversation the binding names, which is
//!   why the binding table is read from the channel side here as well.

use cowbot_core::channel::{ChannelId, ChannelMessage, Sender};
use cowbot_core::error::CowResult;
use cowbot_core::event::EventPayload;
use cowbot_core::integration::IntegrationRecord;
use cowbot_store::Store;

use crate::model::{ChunkLimit, ChunkPart, OutboundContent, OutboundTarget};

/// Longest message most platforms accept, in characters.
///
/// Mattermost and Slack both cap around 4 000, Telegram at 4 096 for a text
/// message, and Feishu at 30 000 but with cards that read poorly past a screen.
/// Splitting to the smallest common limit keeps one answer looking the same
/// everywhere, at the cost of a few extra messages on Feishu.
pub const MAX_CHUNK_CHARS: usize = 4_000;

/// Split an answer into platform-sized chunks.
///
/// Splits on a paragraph boundary, then a line boundary, then a sentence, and
/// only then mid-word — a code block cut mid-line is unreadable, but an
/// over-limit message is refused outright, so something has to give.
///
/// The budget counts characters, not bytes: a platform's limit counts
/// characters, and a byte limit would split an answer written in Chinese far
/// earlier than the platform requires. A platform that counts UTF-16 code units
/// instead — Telegram does — calls [`chunk_units`].
pub fn chunk(text: &str, limit: usize) -> Vec<String> {
    split_by(text, limit, |_| 1)
}

/// Split to a limit counted in UTF-16 code units.
///
/// An emoji is two units and a character, so a 4 000-character answer is 4 000
/// units only when nothing in it is outside the basic plane. A platform whose
/// limit is in units refuses the all-emoji version, which is why the split has
/// to measure the way the platform does.
pub fn chunk_units(text: &str, limit: usize) -> Vec<String> {
    split_by(text, limit, char::len_utf16)
}

fn split_by(text: &str, limit: usize, width: impl Fn(char) -> usize) -> Vec<String> {
    let limit = limit.max(1);
    let characters: Vec<char> = text.chars().collect();
    // Cumulative units, so each step can find its window without re-measuring
    // the rest of the answer.
    let mut units = Vec::with_capacity(characters.len() + 1);
    units.push(0usize);
    for character in &characters {
        units.push(units.last().copied().unwrap_or(0) + width(*character));
    }
    let total = units[characters.len()];

    if total <= limit {
        let trimmed = text.trim();
        return if trimmed.is_empty() {
            Vec::new()
        } else {
            vec![trimmed.to_string()]
        };
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut start = 0;
    while start < characters.len() {
        let remaining = total - units[start];
        if remaining <= limit {
            let tail: String = characters[start..].iter().collect();
            let trimmed = tail.trim_end();
            if !trimmed.is_empty() {
                chunks.push(trimmed.to_string());
            }
            break;
        }

        // The longest prefix of what is left that fits. At least one character,
        // so a platform limit narrower than a single code point still advances.
        let fits = units[start..].partition_point(|used| *used - units[start] <= limit);
        let window_len = fits.saturating_sub(1).clamp(1, characters.len() - start);
        let window = &characters[start..start + window_len];
        let split = boundary(window).unwrap_or(window_len);
        let piece: String = characters[start..start + split].iter().collect();
        let trimmed = piece.trim_end();
        if !trimmed.is_empty() {
            chunks.push(trimmed.to_string());
        }
        start += split;
        // The next chunk starts at the first non-space character, so a split on
        // a blank line does not leave every following chunk indented by one.
        while start < characters.len() && characters[start].is_whitespace() {
            start += 1;
        }
    }
    chunks
}

/// Where to cut a full window, preferring a structural boundary.
///
/// Returns `None` when the window has no boundary at all, which leaves the cut
/// at the limit.
fn boundary(window: &[char]) -> Option<usize> {
    // A blank line, the strongest boundary: a new paragraph. The index has to be
    // greater than zero — a window that *starts* with one would otherwise cut
    // nothing and leave the caller's cursor where it was.
    if let Some(index) = find_last(window, |character| *character == '\n')
        .filter(|index| *index > 0)
        .and_then(|index| double_newline_before(window, index))
        .filter(|index| *index > 0)
    {
        return Some(index);
    }
    // A single newline: a new line or bullet.
    if let Some(index) =
        find_last(window, |character| *character == '\n').filter(|index| *index > 0)
    {
        return Some(index + 1);
    }
    // A sentence end, so a long paragraph breaks between sentences.
    if let Some(index) = find_last(window, is_sentence_end).filter(|index| *index > 0) {
        return Some(index + 1);
    }
    // A space, so words stay whole.
    find_last(window, |character| character.is_whitespace()).filter(|index| *index > 0)
}

/// The index at which a paragraph break ending at `newline` starts cutting.
///
/// Cutting before the blank line leaves the following chunk starting on a
/// paragraph, which reads correctly in every platform's renderer.
fn double_newline_before(window: &[char], newline: usize) -> Option<usize> {
    let previous = window[..newline]
        .iter()
        .rposition(|character| *character != '\r')?;
    if window[previous] == '\n' {
        Some(previous)
    } else {
        None
    }
}

/// The highest index in `window` whose character satisfies `accept`.
fn find_last(window: &[char], accept: impl Fn(&char) -> bool) -> Option<usize> {
    window.iter().rposition(accept)
}

fn is_sentence_end(character: &char) -> bool {
    matches!(character, '.' | '!' | '?' | '。' | '！' | '？')
}

/// Prefix a multi-part answer with its position.
///
/// A chunked reply arrives as several messages in a row; saying which is which
/// is the difference between a long answer and what reads as the bot talking to
/// itself. A single-chunk answer is left untouched, because "(1/1)" on every
/// reply would be noise.
///
/// Shared rather than repeated per adapter: the numbering is about the answer,
/// not about the platform, and two adapters formatting it differently would make
/// the same reply read differently in a Mattermost channel and a Telegram chat.
pub fn label_chunk(content: &OutboundContent) -> String {
    if !content.part.is_multi() {
        return content.text.clone();
    }
    format!(
        "({}/{})\n{}",
        content.part.index + 1,
        content.part.total,
        content.text
    )
}

/// One outbound post: which conversation, and which chunk of the answer.
#[derive(Debug, Clone)]
pub struct OutboundPost {
    pub target: OutboundTarget,
    pub content: OutboundContent,
}

/// The message a delivered event carries, when it is one worth mirroring.
///
/// Returns the message so the caller does not have to re-match the payload.
pub fn mirrorable(event: &EventPayload) -> Option<&ChannelMessage> {
    match event {
        EventPayload::MessagePosted(message) if matches!(message.sender, Sender::Agent(_)) => {
            Some(message)
        }
        _ => None,
    }
}

/// Build the posts that mirror one channel message back to the IM.
///
/// Empty when the message is not mirrored: a user's own message is already in
/// the IM, a system notice is CowBot's own commentary, and a channel no binding
/// points at has nowhere to go.
pub async fn mirror_posts(
    store: &Store,
    integration: &IntegrationRecord,
    message: &ChannelMessage,
    limit: ChunkLimit,
) -> CowResult<Vec<OutboundPost>> {
    if !matches!(message.sender, Sender::Agent(_)) {
        return Ok(Vec::new());
    }
    let Some(binding) = store
        .integration_binding_for_channel(&integration.id, &message.channel_id)
        .await?
    else {
        return Ok(Vec::new());
    };

    Ok(build_posts_with(
        integration,
        &binding.external_id,
        message.thread_id.as_deref(),
        &message.content,
        limit,
    ))
}

/// Whether a channel has a binding.
pub async fn mirrors_channel(
    store: &Store,
    integration: &IntegrationRecord,
    channel_id: &ChannelId,
) -> CowResult<bool> {
    Ok(store
        .integration_binding_for_channel(&integration.id, channel_id)
        .await?
        .is_some())
}

/// Build the chunked posts for one answer.
///
/// Free of the store so the chunk labelling can be tested on its own.
pub fn build_posts(
    integration: &IntegrationRecord,
    external_conversation_id: &str,
    thread_id: Option<&str>,
    content: &str,
) -> Vec<OutboundPost> {
    build_posts_with(
        integration,
        external_conversation_id,
        thread_id,
        content,
        ChunkLimit::Characters(MAX_CHUNK_CHARS),
    )
}

/// Build the posts for one answer, split to a platform's own limit.
///
/// An adapter whose platform limit is not [`MAX_CHUNK_CHARS`] passes its own.
/// The split still happens here rather than in the adapter: one answer has to
/// arrive in order and split on the same boundaries on every platform, and only
/// the *unit* differs.
pub fn build_posts_with(
    integration: &IntegrationRecord,
    external_conversation_id: &str,
    thread_id: Option<&str>,
    content: &str,
    limit: ChunkLimit,
) -> Vec<OutboundPost> {
    let chunks = limit.split(content);
    let total = chunks.len();
    let target = OutboundTarget::new(&integration.id, external_conversation_id)
        .in_thread(thread_id.map(str::to_string));

    chunks
        .into_iter()
        .enumerate()
        .map(|(index, text)| OutboundPost {
            target: target.clone(),
            content: OutboundContent {
                text,
                thread_id: thread_id.map(str::to_string),
                part: ChunkPart { index, total },
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowbot_core::agent::AgentId;
    use cowbot_core::identity::WorkspaceId;
    use cowbot_core::integration::{IntegrationId, IntegrationKind};

    fn integration() -> IntegrationRecord {
        IntegrationRecord {
            id: IntegrationId::new("i1"),
            workspace_id: WorkspaceId::new("w1"),
            kind: IntegrationKind::Slack,
            label: "Support".into(),
            created_at: chrono::Utc::now(),
            revoked_at: None,
        }
    }

    #[test]
    fn a_short_answer_is_one_chunk() {
        assert_eq!(chunk("Hello", 4_000), vec!["Hello".to_string()]);
    }

    #[test]
    fn an_answer_at_the_limit_is_not_split() {
        let text = "a".repeat(4_000);
        assert_eq!(chunk(&text, 4_000).len(), 1);
    }

    #[test]
    fn an_empty_answer_produces_no_post() {
        assert!(chunk("   \n ", 4_000).is_empty());
        assert!(build_posts(&integration(), "C024", None, "").is_empty());
    }

    #[test]
    fn a_long_answer_splits_on_a_paragraph() {
        let first = "a".repeat(2_500);
        let second = "b".repeat(2_500);
        let chunks = chunk(&format!("{first}\n\n{second}"), 3_000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], first);
        assert_eq!(chunks[1], second);
    }

    #[test]
    fn every_chunk_fits_the_limit_including_multibyte_text() {
        // A platform counts characters. A byte budget would split Chinese far
        // earlier than the platform requires, and could split mid-codepoint.
        let text = "部署已经完成，请检查日志。".repeat(400);
        for piece in chunk(&text, 500) {
            assert!(
                piece.chars().count() <= 500,
                "{} chars",
                piece.chars().count()
            );
        }
    }

    #[test]
    fn a_paragraph_without_any_boundary_still_splits() {
        let text = "a".repeat(9_000);
        let chunks = chunk(&text, 4_000);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|piece| piece.chars().count() <= 4_000));
        // Nothing was lost.
        assert_eq!(chunks.concat().len(), 9_000);
    }

    #[test]
    fn a_split_on_a_blank_line_does_not_leave_leading_space() {
        let text = format!("{}\n\n{}", "a".repeat(100), "b".repeat(100));
        let chunks = chunk(&text, 105);
        assert!(!chunks[1].starts_with('\n'));
        assert!(!chunks[1].starts_with(' '));
    }

    #[test]
    fn sentences_are_preferred_over_splitting_a_word() {
        let text = format!("{}{}", "a".repeat(90), ". then more words here and there");
        let chunks = chunk(&text, 100);
        assert!(chunks[0].ends_with('.'), "{:?}", chunks[0]);
    }

    #[test]
    fn a_platform_that_counts_units_gets_a_split_in_units() {
        // The wiring that makes `chunk_units` reachable: an emoji-heavy answer
        // is twice as long to Telegram as it is in characters.
        let text = "🚀".repeat(3_000);
        let posts = build_posts_with(&integration(), "7", None, &text, ChunkLimit::Units(4_096));
        assert_eq!(posts.len(), 2);
        for post in &posts {
            assert!(post.content.text.encode_utf16().count() <= 4_096);
        }
    }

    #[test]
    fn a_long_answer_is_labelled_with_its_position() {
        let posts = build_posts(&integration(), "C024", Some("t1"), &"a".repeat(9_000));
        assert_eq!(posts.len(), 3);
        assert_eq!(posts[0].content.part, ChunkPart { index: 0, total: 3 });
        assert_eq!(posts[2].content.part, ChunkPart { index: 2, total: 3 });
        assert!(posts[0].content.part.is_multi());
        assert_eq!(posts[0].target.thread_id.as_deref(), Some("t1"));
        assert_eq!(posts[0].target.external_conversation_id, "C024");
    }

    #[test]
    fn a_one_chunk_answer_is_not_labelled_as_part_of_an_answer() {
        let posts = build_posts(&integration(), "C024", None, "Hello");
        assert_eq!(posts.len(), 1);
        assert!(!posts[0].content.part.is_multi());
    }

    #[test]
    fn a_user_message_is_not_a_mirror_candidate() {
        let user = ChannelMessage::new(ChannelId::new("c1"), Sender::User("u1".into()), "hello");
        assert!(mirrorable(&EventPayload::MessagePosted(user)).is_none());

        let agent = ChannelMessage::new(
            ChannelId::new("c1"),
            Sender::Agent(AgentId("a1".into())),
            "hello",
        );
        assert!(mirrorable(&EventPayload::MessagePosted(agent)).is_some());
    }

    #[test]
    fn a_channel_created_event_is_not_a_mirror_candidate() {
        assert!(mirrorable(&EventPayload::StreamResumed).is_none());
    }

    #[test]
    fn an_emoji_costs_two_units_when_the_limit_is_counted_in_them() {
        // The reason `chunk_units` exists: 4 096 characters of emoji are 8 192
        // UTF-16 units, and Telegram counts units. A character-counted split
        // would hand it a message it refuses.
        let text = "🚀".repeat(100);
        let chunks = chunk_units(&text, 64);
        assert_eq!(chunks.len(), 4);
        for piece in &chunks {
            assert!(
                piece.encode_utf16().count() <= 64,
                "{} units",
                piece.encode_utf16().count()
            );
        }
        assert_eq!(chunks.concat().chars().count(), 100, "text was lost");
    }

    #[test]
    fn a_plain_answer_splits_the_same_way_in_characters_and_units() {
        // Nothing outside the basic plane, so both budgets agree and a user sees
        // no difference between the two code paths.
        let text = "deploy is green. ".repeat(500);
        assert_eq!(chunk(&text, 1000), chunk_units(&text, 1000));
    }

    #[test]
    fn a_limit_narrower_than_one_emoji_still_advances() {
        // A pathological limit must not hang the loop.
        let chunks = chunk_units("🚀🚀🚀", 1);
        assert_eq!(chunks.len(), 3);
    }

    #[test]
    fn an_answer_that_starts_with_a_blank_line_after_a_split_does_not_stall() {
        // The window can *start* on a paragraph break; cutting at index 0 would
        // never advance and the loop would spin forever.
        let text = format!("{}\n\n{}", "a".repeat(60), "b".repeat(200));
        let chunks = chunk(&text, 100);
        assert_eq!(chunks.concat().len(), 260);
    }

    #[test]
    fn the_chunk_label_is_the_same_on_every_platform() {
        let single = OutboundContent {
            text: "Hello".into(),
            thread_id: None,
            part: ChunkPart { index: 0, total: 1 },
        };
        assert_eq!(label_chunk(&single), "Hello");

        let second = OutboundContent {
            text: "second half".into(),
            thread_id: None,
            part: ChunkPart { index: 1, total: 3 },
        };
        assert_eq!(label_chunk(&second), "(2/3)\nsecond half");
    }
}
