//! Asking a person in an IM to decide an approval, and taking their answer
//! (ADR-0060).
//!
//! The approval machinery already has the shape this needs, which is why the
//! waiting turn does not change: `ApprovalService::authorize` publishes
//! [`EventPayload::ApprovalRequested`] and then re-reads the row until it is
//! decided, and `ApprovalService::decide` updates the row and publishes
//! [`EventPayload::ApprovalDecided`]. So an adapter needs no resolver handle and
//! no rewritten wait loop — it needs to notice a request for a conversation it
//! serves, and to call `decide` when somebody taps a button.
//!
//! What lives here is the part that is the same on every platform: deciding
//! which requests an integration should show, turning a bus event into the
//! prompt an adapter renders, and attributing the answer to the IM account that
//! gave it rather than to the bot.

use cowbot_core::approval::{Approval, ApprovalId, ApprovalStatus};
use cowbot_core::error::CowResult;
use cowbot_core::event::EventPayload;
use cowbot_core::integration::IntegrationRecord;
use cowbot_core::risk::RiskLevel;
use cowbot_core::task::TaskId;
use cowbot_core::tool::ToolCall;
use cowbot_service::ApprovalService;
use cowbot_store::Store;

use crate::adapter::IntegrationContext;
use crate::model::ExternalAuthor;
use crate::principal::resolve_principal;

/// How much of a tool call's arguments a prompt shows.
///
/// A pending `write_file` carries the whole file in its arguments. Posting that
/// into an IM is unreadable and a data-egress problem both, and a person
/// deciding needs the shape of the call rather than its payload.
pub const ARGUMENT_PREVIEW_CHARS: usize = 600;

/// One approval an adapter should ask a person about.
///
/// Deliberately platform-neutral and already reduced to text: every platform
/// needs the same four facts, and three adapters formatting them differently
/// would make the same decision look like a different one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPrompt {
    /// The approval's own id, which is what a button carries back.
    ///
    /// It has to fit a platform's callback payload — Telegram allows 64 bytes —
    /// which is why it is the id itself rather than a shorter form that could
    /// collide with another pending decision.
    pub approval_id: String,
    pub tool_name: String,
    /// The arguments, truncated to [`ARGUMENT_PREVIEW_CHARS`].
    pub arguments: String,
    pub risk: RiskLevel,
    pub reason: String,
    /// The CowBot task the call belongs to, so a person can tell two concurrent
    /// runs in one room apart.
    pub task_id: String,
}

impl ApprovalPrompt {
    /// Build the prompt from the bus event that announced the request.
    ///
    /// Returns `None` for any other event, so a caller can hand it every
    /// payload it reads.
    pub fn from_event(payload: &EventPayload) -> Option<Self> {
        let EventPayload::ApprovalRequested {
            approval,
            risk,
            reason,
            call,
            ..
        } = payload
        else {
            return None;
        };
        let mut prompt = Self {
            approval_id: approval.id.0.clone(),
            // The call id is what there is when the event predates the `call`
            // field; showing it beats showing nothing.
            tool_name: approval.tool_call_id.clone(),
            arguments: String::new(),
            risk: *risk,
            reason: reason.clone(),
            task_id: approval.task_id.0.clone(),
        };
        if let Some(call) = call {
            prompt = prompt.with_call(call);
        }
        Some(prompt)
    }

    /// The same prompt with the call's name and arguments filled in.
    pub fn with_call(mut self, call: &ToolCall) -> Self {
        self.tool_name = call.name.clone();
        self.arguments = preview(&call.arguments.to_string());
        self
    }

    /// The text every platform's card shows, before its own formatting.
    pub fn describe(&self) -> String {
        let mut text = format!(
            "Approval needed: {} ({})\n{}",
            self.tool_name,
            self.risk.as_str(),
            self.reason
        );
        if !self.arguments.is_empty() {
            text.push_str("\n\n");
            text.push_str(&self.arguments);
        }
        text
    }
}

/// Truncate a tool call's arguments for display, on a character boundary.
pub fn preview(arguments: &str) -> String {
    let trimmed = arguments.trim();
    match trimmed.char_indices().nth(ARGUMENT_PREVIEW_CHARS) {
        Some((index, _)) => format!("{}…", &trimmed[..index]),
        None => trimmed.to_string(),
    }
}

/// Whether this integration should ask a person about `payload`, and where.
///
/// True when the event is an approval request whose task belongs to a channel
/// this integration is bound to. Resolving it here rather than in each adapter
/// means no adapter can post a card into a conversation it does not serve —
/// which would show one room's approval request to whoever is in another.
pub async fn prompt_for(
    store: &Store,
    integration: &IntegrationRecord,
    payload: &EventPayload,
) -> CowResult<Option<(ApprovalPrompt, String)>> {
    let Some(prompt) = ApprovalPrompt::from_event(payload) else {
        return Ok(None);
    };
    let Some(task) = store.get_task(&TaskId(prompt.task_id.clone())).await? else {
        return Ok(None);
    };
    let Some(channel_id) = task.channel_id else {
        // A task started from the task view belongs to no channel, so there is
        // no conversation to ask in. The desktop's own card is the right surface.
        return Ok(None);
    };
    let Some(binding) = store
        .integration_binding_for_channel(&integration.id, &channel_id)
        .await?
    else {
        return Ok(None);
    };
    Ok(Some((prompt, binding.external_id)))
}

/// What happened when somebody decided an approval from an IM.
#[derive(Debug, Clone)]
pub struct Decided {
    pub approval: Approval,
    /// The name of the account that decided, for the card the adapter updates.
    pub decided_by: String,
}

/// How an adapter posted an approval request.
///
/// Reported so the mirror knows whether to revise the message when the decision
/// lands, and so a test can tell a real button from the text fallback. It is not
/// a decision: the decision arrives through the store, whenever the person makes
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPosting {
    /// The message the adapter posted, so it can be revised in place.
    pub delivered: crate::model::DeliveredId,
    /// Whether the posting drew buttons whose tap comes back to the adapter.
    ///
    /// False means the person can read the question in the IM and has to decide
    /// somewhere else, which is a real answer rather than a dead button.
    pub buttons: bool,
}

/// The approval a decision event settles, when it is one this integration could
/// have shown.
///
/// The mirror uses this to revise the card it drew. A decision for a task in no
/// channel, or in a channel this integration is not bound to, is one it never
/// posted, and revising nothing is the right answer for it.
pub async fn decided_for(
    store: &Store,
    integration: &IntegrationRecord,
    payload: &EventPayload,
) -> CowResult<Option<Approval>> {
    let EventPayload::ApprovalDecided { approval } = payload else {
        return Ok(None);
    };
    if approval.status == ApprovalStatus::Pending {
        return Ok(None);
    }
    let Some(task) = store.get_task(&approval.task_id).await? else {
        return Ok(None);
    };
    let Some(channel_id) = task.channel_id else {
        return Ok(None);
    };
    let bound = store
        .integration_binding_for_channel(&integration.id, &channel_id)
        .await?;
    Ok(bound.map(|_| approval.clone()))
}

/// How a decided approval reads on the card that asked.
///
/// One sentence per platform would make the same outcome read differently in
/// two rooms, so it is written once.
pub fn decision_summary(approval: &Approval) -> String {
    match approval.status {
        ApprovalStatus::Approved => format!(
            "Approved by {}.",
            approval.decided_by.as_deref().unwrap_or("somebody")
        ),
        ApprovalStatus::Rejected => format!(
            "Rejected by {}.",
            approval.decided_by.as_deref().unwrap_or("somebody")
        ),
        ApprovalStatus::Expired => "This approval expired without a decision.".to_string(),
        ApprovalStatus::Pending => "Still waiting for a decision.".to_string(),
    }
}

/// Why a decision from the IM was refused.
///
/// A refusal is not an error to log and forget: the person who tapped is looking
/// at a card that did nothing, so the adapter has to be able to say
/// "you cannot decide this one" in as many words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The row was no longer pending: it had already been decided, or it
    /// expired, or this is a second tap on one card.
    NotPending(String),
    /// This account is not allowed to decide this request.
    NotAllowed(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::NotPending(reason) | Refused::NotAllowed(reason) => {
                formatter.write_str(reason)
            }
        }
    }
}

/// Whether `user_id` may decide `approval`.
///
/// **This is the check that makes an IM approval worth anything.** A card sits in
/// a room, and every role can write (`Role::can_write` is `true` for all three),
/// so without this one person's `execute_code` request could be approved by
/// anybody who happens to be in the channel — which turns a prompt injection into
/// a one-tap compromise.
///
/// A decision is allowed for the person who asked, and for a workspace
/// administrator. The requester is the assignment's, which the channel turn
/// records as the CowBot user whose message started it; a direct conversation
/// provisioned from the same IM account is therefore decidable by that account
/// and nobody else.
pub async fn may_decide(
    store: &Store,
    workspace_id: &cowbot_core::identity::WorkspaceId,
    approval: &Approval,
    user_id: &str,
) -> CowResult<bool> {
    let member = store
        .get_membership(&cowbot_core::identity::UserId::new(user_id), workspace_id)
        .await?;
    if member.is_some_and(|member| member.role.can_administer()) {
        return Ok(true);
    }
    // The requester of the task's most recent assignment. A task with no
    // assignment yet has nobody who asked, and then only an administrator may
    // decide it.
    let assignments = store.list_assignments_for_task(&approval.task_id).await?;
    let requester = assignments
        .iter()
        .rev()
        .find_map(|assignment| match &assignment.requester {
            cowbot_core::assignment::AssignmentRequester::User(id) => Some(id.clone()),
            _ => None,
        });
    Ok(requester.is_some_and(|requester| requester == user_id))
}

/// Decide an approval on behalf of the IM account that tapped the button.
///
/// The decision is attributed to that account's CowBot user, never to the bot:
/// `tool_calls.approved_by` and the audit trail are read by people asking who
/// allowed this, and "the bot allowed itself" is the answer that makes the
/// feature worthless. The id — not the display name — is what is stored, because
/// every existing caller does the same and the trail joins on it.
pub async fn decide_from_im(
    ctx: &IntegrationContext,
    approval_id: &str,
    approved: bool,
    author: &ExternalAuthor,
) -> Result<Decided, Refused> {
    let principal = resolve_principal(&ctx.store, &ctx.integration, author)
        .await
        .map_err(|error| Refused::NotPending(error.to_string()))?;
    let approval = ctx
        .store
        .get_approval(&ApprovalId::new(approval_id))
        .await
        .map_err(|error| Refused::NotPending(error.to_string()))?
        .ok_or_else(|| Refused::NotPending("that approval no longer exists".into()))?;

    let allowed = may_decide(
        &ctx.store,
        &ctx.integration.workspace_id,
        &approval,
        &principal.user_id.0,
    )
    .await
    .map_err(|error| Refused::NotPending(error.to_string()))?;
    if !allowed {
        return Err(Refused::NotAllowed(format!(
            "{} cannot decide this one; only the person who asked and a workspace              administrator can",
            principal.display_name
        )));
    }

    let service = ApprovalService::new(
        ctx.store.clone(),
        ctx.bus.clone(),
        ctx.integration.workspace_id.clone(),
    );
    let status = if approved {
        ApprovalStatus::Approved
    } else {
        ApprovalStatus::Rejected
    };
    let approval = service
        .decide(&ApprovalId::new(approval_id), status, &principal.user_id.0)
        .await
        .map_err(|error| Refused::NotPending(error.to_string()))?;
    Ok(Decided {
        approval,
        decided_by: principal.display_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }

    #[test]
    fn a_long_argument_payload_is_cut_on_a_character_boundary() {
        let arguments = "é".repeat(ARGUMENT_PREVIEW_CHARS + 50);
        let cut = preview(&arguments);
        assert_eq!(cut.chars().count(), ARGUMENT_PREVIEW_CHARS + 1);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn a_short_argument_payload_is_shown_whole() {
        assert_eq!(preview("  {\"path\":\"a\"}  "), "{\"path\":\"a\"}");
    }

    /// A workspace with one task and one assignment, so `may_decide` has a
    /// requester to look for.
    async fn workspace_with_requester(
        requester: &str,
    ) -> (
        Store,
        cowbot_core::identity::WorkspaceId,
        cowbot_core::task::TaskId,
    ) {
        use cowbot_core::agent::AgentProfile;
        use cowbot_core::approval::ApprovalLevel;
        use cowbot_core::assignment::{
            Assignment, AssignmentMode, AssignmentOrigin, AssignmentRequester, ReplyMode,
        };
        use cowbot_core::identity::Workspace;
        use cowbot_core::task::Task;

        let store = Store::connect_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let workspace = Workspace::new("Team");
        store.create_workspace(&workspace).await.unwrap();

        let agent = AgentProfile::new("CowBot", "assistant", "answer", "a test agent");
        store.upsert_agent(&workspace.id, &agent).await.unwrap();
        let task = Task::new("Deploy", "Output");
        store.create_task(&workspace.id, None, &task).await.unwrap();

        let assignment = Assignment::new_root(
            task.id.clone(),
            AssignmentOrigin::User,
            AssignmentRequester::User(requester.to_string()),
            agent.id,
            AssignmentMode::Work,
            ReplyMode::Poll,
            ApprovalLevel::Ask,
        );
        store.create_assignment(&assignment).await.unwrap();
        (store, workspace.id, task.id)
    }

    #[tokio::test]
    async fn only_the_person_who_asked_or_an_administrator_may_decide() {
        // The check that makes an IM approval worth anything: a card sits in a
        // shared room and every role can write, so without this anyone present
        // could approve somebody else's `execute_code`.
        let (store, workspace_id, task_id) = workspace_with_requester("user-1").await;
        let approval = Approval::new(task_id, "call-1");

        assert!(
            may_decide(&store, &workspace_id, &approval, "user-1")
                .await
                .unwrap(),
            "the person who asked may decide"
        );
        assert!(
            !may_decide(&store, &workspace_id, &approval, "someone-else")
                .await
                .unwrap(),
            "a bystander may not"
        );
    }

    #[tokio::test]
    async fn an_administrator_may_decide_anybodys_request() {
        use cowbot_core::identity::UserId;
        use cowbot_core::identity::{Membership, Role, User};

        let (store, workspace_id, task_id) = workspace_with_requester("user-1").await;
        let admin = UserId::new("admin-1");
        store
            .create_user(&User {
                id: admin.clone(),
                name: "Admin".into(),
                email: "admin@example.test".into(),
                password_hash: String::new(),
                created_at: chrono::Utc::now(),
            })
            .await
            .unwrap();
        store
            .add_member(&Membership::new(
                admin.clone(),
                workspace_id.clone(),
                Role::Admin,
            ))
            .await
            .unwrap();

        let approval = Approval::new(task_id, "call-1");
        assert!(may_decide(&store, &workspace_id, &approval, &admin.0)
            .await
            .unwrap());
        // A plain Member who is not the requester still may not.
        assert!(!may_decide(&store, &workspace_id, &approval, "member-2")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn a_request_from_a_system_origin_has_nobody_who_may_decide_in_the_room() {
        // No requester means no owner of the request, so only an administrator
        // can answer it. Reporting `false` rather than falling open is the whole
        // point.
        let store = Store::connect_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let workspace = cowbot_core::identity::Workspace::new("Team");
        store.create_workspace(&workspace).await.unwrap();
        let task = cowbot_core::task::Task::new("Deploy", "Output");
        store.create_task(&workspace.id, None, &task).await.unwrap();

        let approval = Approval::new(task.id, "call-1");
        assert!(!may_decide(&store, &workspace.id, &approval, "anyone")
            .await
            .unwrap());
    }

    #[test]
    fn an_event_that_is_not_an_approval_request_is_not_a_prompt() {
        assert!(ApprovalPrompt::from_event(&EventPayload::StreamResumed).is_none());
    }

    #[test]
    fn the_announced_call_supplies_the_name_and_arguments() {
        // The event carries `tool_call_id`; the agent loop announced the call
        // itself on the bus one event earlier, which is where the rest comes
        // from.
        let payload = EventPayload::ApprovalRequested {
            approval: Approval::new(TaskId("t1".into()), "call-1"),
            risk: RiskLevel::High,
            reason: "writes outside the workspace".into(),
            rule: None,
            call: Some(call("call-1", "write_file", json!({"path": "a.txt"}))),
        };
        let prompt = ApprovalPrompt::from_event(&payload).expect("it is a request");
        assert_eq!(prompt.tool_name, "write_file");
        assert!(prompt.arguments.contains("a.txt"));
        let text = prompt.describe();
        assert!(text.contains("write_file"));
        assert!(text.contains("high"));
        assert!(text.contains("writes outside the workspace"));

        // An event from a build that predates the `call` field still produces a
        // prompt, naming the call by id.
        let older = EventPayload::ApprovalRequested {
            approval: Approval::new(TaskId("t1".into()), "call-1"),
            risk: RiskLevel::High,
            reason: "r".into(),
            rule: None,
            call: None,
        };
        assert_eq!(
            ApprovalPrompt::from_event(&older).unwrap().tool_name,
            "call-1"
        );
    }
}
