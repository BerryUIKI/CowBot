//! The Block Kit JSON an approval card is drawn with (ADR-0059).
//!
//! Kept apart from the adapter because a block array is a wire shape, not
//! behaviour: the adapter's job is to decide *when* to draw a card, and this
//! module's is what Slack must receive for it to render. Both revisions are
//! built here so a decided card and its question cannot drift apart.

use serde_json::{json, Value};

/// The question, with an Approve and a Reject button.
///
/// `approval_id` goes into both buttons' `value`, which is all a decision needs:
/// the payload lives in CowBot, and the button is a pointer to it.
pub fn approval(question: &str, approval_id: &str) -> Value {
    json!([
        {
            "type": "section",
            "text": { "type": "mrkdwn", "text": truncate(question, 2_900) }
        },
        {
            "type": "actions",
            "block_id": "approval",
            "elements": [
                {
                    "type": "button",
                    "action_id": "approve",
                    "style": "primary",
                    "text": { "type": "plain_text", "text": "Approve" },
                    "value": crate::adapter::action_value(approval_id, true)
                },
                {
                    "type": "button",
                    "action_id": "reject",
                    "style": "danger",
                    "text": { "type": "plain_text", "text": "Reject" },
                    "value": crate::adapter::action_value(approval_id, false)
                }
            ]
        }
    ])
}

/// The same card once decided, with no buttons.
///
/// The actions block is dropped rather than disabled: a greyed-out button still
/// invites a tap, and an approval that already landed is not an interactive thing.
pub fn decided(summary: &str) -> Value {
    json!([
        {
            "type": "section",
            "text": { "type": "mrkdwn", "text": truncate(summary, 2_900) }
        }
    ])
}

/// Cut a string to a byte budget and say so.
///
/// Slack's per-block text limit is 3 000 characters and a section that exceeds it
/// is rejected outright, which would turn a long tool-call argument into no card
/// at all. The ellipsis is there because a truncated argument that does not say
/// it was truncated reads as the whole thing.
fn truncate(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((index, _)) => format!("{}…", &text[..index]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_question_carries_an_approve_and_a_reject_button_for_this_approval() {
        let blocks = approval("Approval needed: write_file", "a1");
        let actions = blocks
            .as_array()
            .unwrap()
            .iter()
            .find(|block| block["type"] == "actions")
            .expect("an actions block");
        let elements = actions["elements"].as_array().unwrap();
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0]["value"], "approve:a1");
        assert_eq!(elements[1]["value"], "reject:a1");
        assert_eq!(elements[0]["style"], "primary");
    }

    #[test]
    fn a_decided_card_has_no_buttons_left_to_tap() {
        let blocks = decided("Approved by Ada.");
        let blocks = blocks.as_array().unwrap();
        assert!(!blocks.iter().any(|block| block["type"] == "actions"));
    }

    #[test]
    fn an_argument_longer_than_slacks_block_limit_is_cut_and_says_so() {
        let long = "a".repeat(5_000);
        let blocks = approval(&long, "a1");
        let text = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(text.chars().count() <= 2_901, "{}", text.chars().count());
        assert!(text.ends_with('…'));
    }
}
