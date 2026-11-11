//! `@Agent` mentions in channel messages.
//!
//! Mentions are resolved against the workspace agents by display name. A
//! message may address one agent, several agents, or the whole team.

use crate::agent::AgentProfile;

/// Names that address every agent in the resolved roster.
///
/// A group channel resolves them against the workspace agents; a user-created
/// group conversation resolves them against its participants.
pub const BROADCAST_MENTIONS: &[&str] = &["team", "all", "everyone", "here"];

/// Whether `name` addresses the whole resolved roster.
pub fn is_broadcast_mention(name: &str) -> bool {
    BROADCAST_MENTIONS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(name))
}

/// Agents addressed by a message plus the names that matched nothing.
#[derive(Debug, Clone, Default)]
pub struct MentionResolution {
    /// Matched agents, in the order they were mentioned.
    pub agents: Vec<AgentProfile>,
    /// Mentioned names with no matching agent, in mention order.
    pub unknown: Vec<String>,
}

impl MentionResolution {
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty() && self.unknown.is_empty()
    }
}

/// Extract the raw mention names in a message.
///
/// A mention starts with `@` at the beginning of the message or after a
/// non-word character, so addresses such as `user@example.com` are ignored.
/// Names keep letters, digits, `_`, and `-`; the result is deduplicated
/// case-insensitively and keeps the original spelling and order.
pub fn parse_mentions(content: &str) -> Vec<String> {
    let mut mentions: Vec<String> = Vec::new();
    let mut chars = content.chars().peekable();
    let mut previous: Option<char> = None;

    while let Some(ch) = chars.next() {
        if ch != '@' {
            previous = Some(ch);
            continue;
        }
        if previous.is_some_and(|previous| {
            previous.is_alphanumeric() || previous == '_' || previous == '@'
        }) {
            previous = Some(ch);
            continue;
        }

        let mut name = String::new();
        while let Some(next) = chars.peek().copied() {
            if next.is_alphanumeric() || next == '_' || next == '-' {
                name.push(next);
                chars.next();
            } else {
                break;
            }
        }
        previous = Some(ch);

        if name.is_empty() {
            continue;
        }
        if !mentions
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&name))
        {
            mentions.push(name);
        }
    }

    mentions
}

/// Resolve the mentions of a message against the workspace agents.
///
/// Names match an agent's display name or id, case-insensitively. When several
/// agents share a display name only the first is used, so a workspace that
/// instantiated the same template twice does not answer twice. `@team`,
/// `@all`, `@everyone`, and `@here` address every distinct agent in the
/// workspace.
pub fn resolve_mentions(content: &str, agents: &[AgentProfile]) -> MentionResolution {
    let mentions = parse_mentions(content);
    let mut resolution = MentionResolution::default();
    if mentions.is_empty() || agents.is_empty() {
        resolution.unknown = mentions;
        return resolution;
    }

    let mut seen: Vec<&str> = Vec::new();
    for mention in mentions {
        if is_broadcast_mention(&mention) {
            for agent in agents {
                if seen
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&agent.name))
                {
                    continue;
                }
                seen.push(&agent.name);
                resolution.agents.push(agent.clone());
            }
            continue;
        }

        let matched = agents.iter().find(|agent| {
            agent.name.eq_ignore_ascii_case(&mention) || agent.id.0.eq_ignore_ascii_case(&mention)
        });
        match matched {
            Some(agent)
                if seen
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&agent.name)) => {}
            Some(agent) => {
                seen.push(&agent.name);
                resolution.agents.push(agent.clone());
            }
            None => resolution.unknown.push(mention),
        }
    }

    resolution
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(name: &str, role: &str) -> AgentProfile {
        AgentProfile::new(name, role, "goal", "backstory")
    }

    #[test]
    fn parses_simple_and_multiple_mentions() {
        assert_eq!(parse_mentions("@Planner help"), vec!["Planner"]);
        assert_eq!(
            parse_mentions("hi @Planner, ask @Cow-Coder about it"),
            vec!["Planner", "Cow-Coder"]
        );
    }

    #[test]
    fn ignores_email_addresses_and_bare_at_signs() {
        assert!(parse_mentions("write to team@example.com").is_empty());
        assert!(parse_mentions("nothing here @").is_empty());
    }

    #[test]
    fn deduplicates_case_insensitively() {
        assert_eq!(parse_mentions("@Planner @planner"), vec!["Planner"]);
    }

    #[test]
    fn resolves_names_case_insensitively_and_reports_unknowns() {
        let agents = vec![agent("Planner", "Architect"), agent("Coder", "Engineer")];

        let resolution = resolve_mentions("@coder and @Nobody", &agents);
        assert_eq!(resolution.agents.len(), 1);
        assert_eq!(resolution.agents[0].name, "Coder");
        assert_eq!(resolution.unknown, vec!["Nobody"]);
    }

    #[test]
    fn broadcast_mentions_address_every_distinct_agent() {
        let agents = vec![
            agent("Planner", "Architect"),
            agent("Planner", "Research Lead"),
            agent("Writer", "Writer"),
        ];

        let resolution = resolve_mentions("@team, status please", &agents);
        let names: Vec<&str> = resolution
            .agents
            .iter()
            .map(|agent| agent.name.as_str())
            .collect();
        assert_eq!(names, vec!["Planner", "Writer"]);
    }

    #[test]
    fn broadcast_mentions_can_be_scoped_to_a_participant_roster() {
        let participants = vec![agent("Planner", "Architect"), agent("Coder", "Engineer")];

        let resolution = resolve_mentions("@team, where are we?", &participants);
        let names: Vec<&str> = resolution
            .agents
            .iter()
            .map(|agent| agent.name.as_str())
            .collect();
        assert_eq!(names, vec!["Planner", "Coder"]);
    }

    #[test]
    fn broadcast_names_are_recognized_case_insensitively() {
        assert!(is_broadcast_mention("Team"));
        assert!(is_broadcast_mention("EVERYONE"));
        assert!(!is_broadcast_mention("Planner"));
    }

    #[test]
    fn messages_without_mentions_resolve_to_nothing() {
        let agents = vec![agent("Planner", "Architect")];
        assert!(resolve_mentions("just chatting", &agents).is_empty());
    }
}
