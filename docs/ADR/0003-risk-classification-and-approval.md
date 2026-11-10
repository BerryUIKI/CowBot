# ADR 0003: Risk Classification and Approval Policy

- **Status**: Accepted
- **Date**: 2026-11-03
- **Related**: `docs/PRD.md` §5.1, `docs/ARCHITECTURE.md` §4.2

## Context

CowBot offers multiple approval modes (Ask / AcceptEdits / SafeAuto / PlanOnly / Trusted) and lets agents execute commands and modify files. Risk classification must be deterministic, auditable, and configurable; the safety boundary cannot be handed entirely to an LLM.

## Decision

1. **Static rules first**: classify operations into five risk levels based on tool type, command pattern, path scope, network access, destructive operations, and sensitive data access:
   - `ReadOnly`: read-only queries, listings, reads;
   - `Low`: reversible writes inside the workspace, formatting;
   - `Medium`: ordinary command execution, dependency installs, config changes;
   - `High`: deletion, force-push, paths outside the workspace, download-and-execute;
   - `Critical`: system-level damage, credential exfiltration, sandbox bypass, bulk irreversible operations.
2. **Rule sources and priority**: built-in defaults plus workspace-specific allow / ask / deny rules. A rule names an exact tool (or `*`) and, for `execute_code`, an optional case-insensitive command glob. Rules are evaluated before the model evaluator; among matching rules the order is **Deny > Ask > Allow**, with the most specific pattern winning a tie. Across the decision path the order is **hard policy denial (Plan mode mutations, Critical outside Bypass) > Deny rule > session decision > Ask rule > Allow rule > model evaluator > mode policy**. Rules are stored per workspace and exported with `.cowpkg`; imported rules start disabled.
3. **Optional LLM classifier**: only for operations unmatched by static rules, returning a risk suggestion. The classifier **may only raise the risk level, never lower the static floor**. Disabled by default; workspaces may enable it (preferring local models).
4. **Approval modes**:
   - `Ask` (default): ReadOnly auto-approves; Low and above require per-call approval; Critical is denied;
   - `AcceptEdits`: ReadOnly auto-approves; `write_file` calls inside the workspace auto-approve; all other calls follow `Ask`; Critical is denied;
   - `SafeAuto`: ReadOnly/Low auto-approve; Medium and above ask the configured model permission evaluator, which returns allow/ask/deny; without a provider they fall back to asking; Critical is denied;
   - `PlanOnly`: only ReadOnly auto-approves; every write or execution is denied, and the agent is told to finish with a plan. The final output is stored with the task; **Approve plan and continue** marks it approved and continues in Manual mode;
   - `Trusted`: every non-Critical call auto-approves. Critical calls follow the workspace **Bypass critical** setting: `Allow` (default) auto-approves, `Ask` shows a red approval card, and `Block` denies. The destructive `delete_channel` workspace action is the exception: it always asks, even under Bypass (#84);
5. **Undecidable operations default to High** (fail-safe), never auto-approve.
6. **Audit**: record the decision source (`rule`, `model`, `session`, `human`, `policy`, or `system`), the matched rule ID when one decided the call, the approver, timestamp, and operation arguments. The audit view filters by source.
7. **Enforcement**: a `ToolGate` is consulted before every tool execution. `ApprovalService` classifies the call, applies the assignment's approval level, creates a pending approval when required, waits for the decision, and writes audit events. Delegated assignments inherit `inherit_approval_level(parent, requested)`, where `PlanOnly` is a hard read-only boundary and otherwise the more permissive level wins in the order `Ask < AcceptEdits < SafeAuto < Trusted`.
8. **Model permission evaluator**: in Auto mode, a mutating tool call is sent to a low-temperature model call with the tool name, truncated arguments, static risk, agent role/goal, and recent tool calls. The evaluator returns `allow`, `ask`, or `deny` with a reason and confidence. Failures, invalid JSON, timeouts, and confidence below 0.7 fall back to asking a human; arguments that look like instructions aimed at the evaluator force a human decision. Critical risk never reaches the evaluator. Every model decision and fallback is recorded in the audit trail with the model id, decision, confidence, and reason.
9. **Bypass safeguards**: selecting Bypass permissions shows a warning every time; the acknowledgment is not remembered across restarts. While Bypass is active, the mode control carries a red marker. A Critical approval card offers only Approve once and Reject, and Critical calls never get an "Approve for this session" option.
10. **Acceptance matrix**: a table test in `cowbot-service` asserts each mode's outcome for read-only, edit inside/outside the workspace, shell at each risk level, workspace actions, and unknown tools, so the implementation cannot drift from this document. `delete_channel` is asserted as always-ask (and denied in Plan) across every mode.

## Rationale

- Static rules are deterministic, low-latency, and explainable, making them suitable as the safety boundary;
- An LLM classifier handles long-tail commands but only as a "raise caution" supplement, avoiding model misjudgment that could grant excessive permissions;
- Multiple modes balance efficiency and safety across workspace trust levels;
- Full auditing satisfies the traceability needs of self-hosted teams.

## Consequences

- A rule library and command parser must be maintained (cross-platform shell differences need testing);
- The LLM classifier adds cost and latency and is disabled by default;
- False positives may increase approval burden; configurable allowlists and "always allow" are required;
- The mapping between risk levels and approval modes must be clear in the UI.

## Alternatives

- **LLM-only classification**: flexible but non-deterministic and vulnerable to prompt injection; not the primary decision maker.
- **Static rules only**: deterministic but poor at covering long-tail commands, hurting UX.
- **Per-tool grading**: simple, but risk varies widely within one tool (read vs. delete).
