//! The interactive card an approval is asked with (ADR-0061).
//!
//! Feishu's card JSON 2.0 is a tree of `body.elements`, and an approval is two
//! `button` elements whose `behaviors` carry the answer and the approval id. The
//! payload is small on purpose: the same decision has to be readable by a
//! Telegram button (64 bytes) and a Slack action, so nothing here depends on
//! Feishu's more generous limits.
//!
//! `update_multi: true` is set in both revisions. Feishu refuses to update a card
//! that did not declare it, so leaving it off one of them would make the "this was
//! approved" revision fail — a card that keeps its buttons after the decision.

use serde_json::{json, Value};

/// The question, with Approve and Reject buttons.
pub fn approval(question: &str, approval_id: &str) -> Value {
    json!({
        "config": { "update_multi": true, "wide_screen_mode": true },
        "header": {
            "template": "orange",
            "title": { "tag": "plain_text", "content": "Approval needed" }
        },
        "body": {
            "elements": [
                { "tag": "markdown", "content": truncate(question, 2_000) },
                {
                    "tag": "action",
                    "actions": [
                        {
                            "tag": "button",
                            "type": "primary",
                            "text": { "tag": "plain_text", "content": "Approve" },
                            "behaviors": [{
                                "type": "callback",
                                "value": {
                                    "application": "approve",
                                    "approval_id": approval_id
                                }
                            }]
                        },
                        {
                            "tag": "button",
                            "type": "danger",
                            "text": { "tag": "plain_text", "content": "Reject" },
                            "behaviors": [{
                                "type": "callback",
                                "value": {
                                    "application": "reject",
                                    "approval_id": approval_id
                                }
                            }]
                        }
                    ]
                }
            ]
        }
    })
}

/// The same card once decided, with no action row.
///
/// The buttons are removed rather than disabled: a card that already settled is
/// not an interactive thing, and a greyed-out button still invites a tap.
pub fn decided(summary: &str) -> Value {
    json!({
        "config": { "update_multi": true, "wide_screen_mode": true },
        "header": {
            "template": "green",
            "title": { "tag": "plain_text", "content": "Approval settled" }
        },
        "body": {
            "elements": [
                { "tag": "markdown", "content": truncate(summary, 2_000) }
            ]
        }
    })
}

/// Cut a string and say so.
fn truncate(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((index, _)) => format!("{}…", &text[..index]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(card: &Value) -> &Vec<Value> {
        card["body"]["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|element| element["tag"] == "action")
            .and_then(|element| element["actions"].as_array())
            .expect("an action row")
    }

    #[test]
    fn the_question_carries_two_callback_buttons_naming_this_approval() {
        let card = approval("Approval needed: write_file", "a1");
        let buttons = actions(&card);
        assert_eq!(buttons.len(), 2);
        assert_eq!(
            buttons[0]["behaviors"][0]["value"]["application"],
            "approve"
        );
        assert_eq!(buttons[1]["behaviors"][0]["value"]["application"], "reject");
        assert_eq!(buttons[0]["behaviors"][0]["value"]["approval_id"], "a1");
    }

    #[test]
    fn both_revisions_declare_update_multi_or_the_decided_one_is_refused() {
        // Feishu will not update a card that did not declare this, so a decided
        // card would fail and keep its buttons.
        assert_eq!(approval("q", "a1")["config"]["update_multi"], true);
        assert_eq!(decided("done")["config"]["update_multi"], true);
    }

    #[test]
    fn a_decided_card_has_no_buttons_left_to_tap() {
        let card = decided("Approved by Ada.");
        let has_actions = card["body"]["elements"]
            .as_array()
            .unwrap()
            .iter()
            .any(|element| element["tag"] == "action");
        assert!(!has_actions);
    }

    #[test]
    fn a_long_question_is_cut_and_says_so() {
        let long = "a".repeat(3_000);
        let card = approval(&long, "a1");
        let text = card["body"]["elements"][0]["content"].as_str().unwrap();
        assert!(text.chars().count() <= 2_001);
        assert!(text.ends_with('…'));
    }
}
