//! Slash commands typed into a composer.
//!
//! A command is a UI action, not a message: the catalogue here names every
//! command, its aliases, and its arguments, and [`parse_command`] turns a line
//! of input into an invocation. Nothing is executed here, and nothing is
//! persisted — the desktop owns behaviour and error copy, while the CLI and
//! future remote clients read the same catalogue and parser so the surface
//! stays identical everywhere.
//!
//! Parsing is pure and synchronous: no state, no store, no service dependency.
//!
//! ```
//! use cowbot_core::{parse_command, CommandParse};
//!
//! match parse_command("/rename Planning") {
//!     CommandParse::Invocation(invocation) => {
//!         assert_eq!(invocation.name, "rename");
//!         assert_eq!(invocation.args, vec!["Planning".to_string()]);
//!     }
//!     other => panic!("expected an invocation, got {other:?}"),
//! }
//! ```

use serde::Serialize;

/// Which composer a command belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandTarget {
    /// A chat: an agent direct conversation or a group channel.
    Chat,
    /// A user-created conversation whose context commands need a branch.
    Conversation,
}

impl CommandTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            CommandTarget::Chat => "chat",
            CommandTarget::Conversation => "conversation",
        }
    }
}

/// The shape of a command argument, used by the desktop for completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandArgKind {
    /// Free text; everything the composer offers comes from the catalogue.
    Text,
    /// An `@name` resolved against the conversation roster by the desktop.
    Participant,
    /// One of a fixed set of values.
    Choice(&'static [&'static str]),
}

impl CommandArgKind {
    /// The wire name of the kind, such as `"choice"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            CommandArgKind::Text => "text",
            CommandArgKind::Participant => "participant",
            CommandArgKind::Choice(_) => "choice",
        }
    }

    /// The accepted values of a [`CommandArgKind::Choice`], otherwise empty.
    pub fn choices(&self) -> &'static [&'static str] {
        match self {
            CommandArgKind::Text | CommandArgKind::Participant => &[],
            CommandArgKind::Choice(values) => values,
        }
    }
}

/// One positional argument of a command.
///
/// Serializes flat — `{"placeholder": "<name>", "required": true, "kind":
/// "text", "choices": []}` — so a TypeScript caller reads `kind` as a string
/// and `choices` as a list without unwrapping a nested tag. A serde-derived
/// internally tagged enum cannot express that shape when one variant carries
/// a slice, which is why the wire form is written by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandArg {
    /// How the argument is written in help copy, such as `"<name>"`.
    pub placeholder: &'static str,
    /// Whether omitting the argument makes the invocation incomplete.
    pub required: bool,
    /// What the argument stands for; drives completion in the desktop.
    pub kind: CommandArgKind,
}

impl Serialize for CommandArg {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("CommandArg", 4)?;
        state.serialize_field("placeholder", self.placeholder)?;
        state.serialize_field("required", &self.required)?;
        state.serialize_field("kind", self.kind.as_str())?;
        state.serialize_field("choices", self.kind.choices())?;
        state.end()
    }
}

/// A command the composer understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CommandSpec {
    /// Canonical name without the leading slash.
    pub name: &'static str,
    /// Other names that resolve to this command.
    pub aliases: &'static [&'static str],
    /// One line showing the command and its arguments, such as `"/compact [@participant]"`.
    pub usage: &'static str,
    /// One line for the command menu.
    pub summary: &'static str,
    /// Positional arguments, in order.
    pub args: &'static [CommandArg],
    /// The composer this command applies to.
    pub target: CommandTarget,
}

impl CommandSpec {
    /// Whether `name` is this command's canonical name or one of its aliases.
    pub fn matches(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
            || self
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(name))
    }

    /// How many leading arguments the command requires.
    ///
    /// Arguments are positional, so a required argument after an optional one
    /// only makes sense once the optional one is filled: the count is the
    /// index after the last required argument, not the number of required
    /// specs. A catalogue that skipped this would accept `/grant @alice` for
    /// `[@participant] <reason>`.
    pub fn min_args(&self) -> usize {
        self.args
            .iter()
            .rposition(|arg| arg.required)
            .map_or(0, |index| index + 1)
    }
}

/// A complete, known command parsed from composer input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CommandInvocation {
    /// The canonical name, lowercase.
    pub name: String,
    /// Quote-aware argument tokens.
    pub args: Vec<String>,
    /// The trimmed text after the command name, quotations included.
    pub raw_args: String,
}

/// The outcome of reading composer input as a slash command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandParse {
    /// The input does not begin with a slash, so it is an ordinary message.
    NotACommand,
    /// A known command with every required argument present.
    Invocation(CommandInvocation),
    /// A slash token that matches no command. An empty name is a bare `/`,
    /// which the desktop keeps as a request to list every command.
    Unknown { name: String },
    /// A known command with a missing required argument.
    MissingArgument { name: String, usage: String },
}

/// Values accepted by `/approval`, in the order the menu presents them.
pub const APPROVAL_CHOICES: &[&str] = &["safe_auto", "ask", "accept_edits", "plan_only", "trusted"];

/// Sections accepted by `/settings`, matching the desktop's Settings view.
pub const SETTINGS_CHOICES: &[&str] = &[
    "model",
    "team",
    "memory",
    "approvals",
    "audit",
    "projects",
    "workspace",
    "about",
];

/// The v1 command catalogue, ordered alphabetically for the command menu.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "approval",
        aliases: &[],
        usage: "/approval [mode]",
        summary: "Set the composer approval mode.",
        args: &[CommandArg {
            placeholder: "safe_auto|ask|accept_edits|plan_only|trusted",
            required: false,
            kind: CommandArgKind::Choice(APPROVAL_CHOICES),
        }],
        target: CommandTarget::Chat,
    },
    CommandSpec {
        name: "btw",
        aliases: &["side"],
        usage: "/btw <question>",
        summary: "Ask a side question; the answer never enters the conversation.",
        args: &[CommandArg {
            placeholder: "<question>",
            required: true,
            kind: CommandArgKind::Text,
        }],
        target: CommandTarget::Chat,
    },
    CommandSpec {
        name: "clear",
        aliases: &[],
        usage: "/clear [@participant]",
        summary: "Clear context after confirmation; messages stay visible.",
        args: &[CommandArg {
            placeholder: "@participant",
            required: false,
            kind: CommandArgKind::Participant,
        }],
        target: CommandTarget::Conversation,
    },
    CommandSpec {
        name: "compact",
        aliases: &[],
        usage: "/compact [@participant]",
        summary: "Compact the active branch or one participant branch.",
        args: &[CommandArg {
            placeholder: "@participant",
            required: false,
            kind: CommandArgKind::Participant,
        }],
        target: CommandTarget::Conversation,
    },
    CommandSpec {
        name: "delete",
        aliases: &[],
        usage: "/delete",
        summary: "Open the delete confirmation for the current chat.",
        args: &[],
        target: CommandTarget::Chat,
    },
    CommandSpec {
        name: "help",
        aliases: &["commands"],
        usage: "/help",
        summary: "List every command.",
        args: &[],
        target: CommandTarget::Chat,
    },
    CommandSpec {
        name: "model",
        aliases: &["models"],
        usage: "/model [name]",
        summary: "Switch the active model, or open the model menu.",
        args: &[CommandArg {
            placeholder: "[name]",
            required: false,
            kind: CommandArgKind::Text,
        }],
        target: CommandTarget::Chat,
    },
    CommandSpec {
        name: "new",
        aliases: &["newchat"],
        usage: "/new [@agent ...]",
        summary: "Open the new-conversation form with participants filled in.",
        args: &[CommandArg {
            placeholder: "@agent",
            required: false,
            kind: CommandArgKind::Participant,
        }],
        target: CommandTarget::Chat,
    },
    CommandSpec {
        name: "note",
        aliases: &[],
        usage: "/note [text]",
        summary: "Save the conversation note, or open the note editor without text.",
        args: &[CommandArg {
            placeholder: "[text]",
            required: false,
            kind: CommandArgKind::Text,
        }],
        target: CommandTarget::Conversation,
    },
    CommandSpec {
        name: "pin",
        aliases: &[],
        usage: "/pin",
        summary: "Pin the current chat.",
        args: &[],
        target: CommandTarget::Chat,
    },
    CommandSpec {
        name: "remember",
        aliases: &[],
        usage: "/remember <text>",
        summary: "Save a memory entry at the scope this chat suggests.",
        args: &[CommandArg {
            placeholder: "<text>",
            required: true,
            kind: CommandArgKind::Text,
        }],
        target: CommandTarget::Chat,
    },
    CommandSpec {
        name: "rename",
        aliases: &[],
        usage: "/rename <name>",
        summary: "Rename the current chat; the topic is unchanged.",
        args: &[CommandArg {
            placeholder: "<name>",
            required: true,
            kind: CommandArgKind::Text,
        }],
        target: CommandTarget::Chat,
    },
    CommandSpec {
        name: "settings",
        aliases: &["prefs", "preferences"],
        usage: "/settings [section]",
        summary: "Open Settings at a section.",
        args: &[CommandArg {
            placeholder: "model|team|memory|approvals|audit|projects|workspace|about",
            required: false,
            kind: CommandArgKind::Choice(SETTINGS_CHOICES),
        }],
        target: CommandTarget::Chat,
    },
    CommandSpec {
        name: "stop",
        aliases: &[],
        usage: "/stop",
        summary: "Cancel the running tasks in this chat after confirmation.",
        args: &[],
        target: CommandTarget::Chat,
    },
    CommandSpec {
        name: "task",
        aliases: &[],
        usage: "/task <description>",
        summary: "Open task creation with the description prefilled.",
        args: &[CommandArg {
            placeholder: "<description>",
            required: true,
            kind: CommandArgKind::Text,
        }],
        target: CommandTarget::Chat,
    },
    CommandSpec {
        name: "unpin",
        aliases: &[],
        usage: "/unpin",
        summary: "Unpin the current chat.",
        args: &[],
        target: CommandTarget::Chat,
    },
];

/// Every command the composer understands, ordered alphabetically.
pub fn command_catalogue() -> &'static [CommandSpec] {
    COMMANDS
}

/// Look a command up by canonical name or alias, case-insensitively.
pub fn find_command(name: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|spec| spec.matches(name))
}

/// Read composer input as a slash command.
///
/// Leading whitespace is ignored, and the command must begin at the start of
/// the input: a slash after other text is ordinary text. The command token is
/// `/` followed by the longest run of `[A-Za-z0-9_-]`, so `/usr/bin/env` reads
/// as the unknown token `usr` rather than a panic. A bare `/` reports an
/// empty unknown name, which the desktop keeps as a request to list every
/// command; a slash followed by text the name grammar rejects reports that
/// whole token, so `/日本語` is an unknown `日本語` rather than an empty name.
///
/// Arguments split on whitespace, and double quotes group words and are
/// stripped. `raw_args` keeps the trimmed remainder verbatim for free-text
/// commands such as `/task`, `/remember`, and `/note`. Only required-argument
/// arity is validated here (see [`CommandSpec::min_args`]); the desktop owns
/// value validation, completion, and error copy.
pub fn parse_command(input: &str) -> CommandParse {
    let rest = match input.trim_start().strip_prefix('/') {
        Some(rest) => rest,
        None => return CommandParse::NotACommand,
    };

    let name_end = rest
        .char_indices()
        .find(|(_, ch)| ch.is_whitespace() || !is_name_char(*ch))
        .map(|(index, _)| index)
        .unwrap_or(rest.len());
    let name = &rest[..name_end];
    let raw_args = rest[name_end..].trim();

    let spec = match find_command(name) {
        Some(spec) => spec,
        None => {
            // A bare `/` lists every command. Any other miss keeps the token
            // the user typed — including a slash followed by text the name
            // grammar rejects — so the desktop can echo it back instead of
            // showing an empty name.
            let unknown = if !name.is_empty() {
                name.to_string()
            } else {
                rest.split(char::is_whitespace)
                    .next()
                    .unwrap_or_default()
                    .to_string()
            };
            return CommandParse::Unknown { name: unknown };
        }
    };

    let args = split_args(raw_args);
    if args.len() < spec.min_args() {
        return CommandParse::MissingArgument {
            name: spec.name.to_string(),
            usage: spec.usage.to_string(),
        };
    }

    CommandParse::Invocation(CommandInvocation {
        name: spec.name.to_string(),
        args,
        raw_args: raw_args.to_string(),
    })
}

fn is_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

/// Split arguments on whitespace, grouping and stripping double quotes.
fn split_args(raw: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut has_token = false;

    for ch in raw.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                has_token = true;
            }
            ch if ch.is_whitespace() && !in_quotes => {
                if has_token {
                    args.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            ch => {
                current.push(ch);
                has_token = true;
            }
        }
    }
    if has_token {
        args.push(current);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation(input: &str) -> CommandInvocation {
        match parse_command(input) {
            CommandParse::Invocation(invocation) => invocation,
            other => panic!("expected an invocation for {input:?}, got {other:?}"),
        }
    }

    #[test]
    fn only_reads_commands_at_the_start_of_the_input() {
        assert_eq!(parse_command("ship it"), CommandParse::NotACommand);
        assert_eq!(parse_command("say /help"), CommandParse::NotACommand);
        assert_eq!(parse_command(""), CommandParse::NotACommand);
        assert_eq!(invocation("   /help").name, "help");
        assert_eq!(invocation("\t/help").name, "help");
    }

    #[test]
    fn resolves_names_case_insensitively_and_through_aliases() {
        assert_eq!(invocation("/HELP").name, "help");
        assert_eq!(invocation("/Commands").name, "help");
        assert_eq!(invocation("/prefs").name, "settings");
        assert_eq!(invocation("/newchat").name, "new");
        assert_eq!(invocation("/models").name, "model");
    }

    #[test]
    fn a_bare_slash_reports_an_empty_unknown_name() {
        assert_eq!(
            parse_command("/"),
            CommandParse::Unknown {
                name: String::new()
            }
        );
        assert_eq!(
            parse_command("   /   "),
            CommandParse::Unknown {
                name: String::new()
            }
        );
    }

    #[test]
    fn reports_unknown_names_without_panicking() {
        assert_eq!(
            parse_command("/nope"),
            CommandParse::Unknown {
                name: "nope".to_string()
            }
        );
        assert_eq!(
            parse_command("/usr/bin/env"),
            CommandParse::Unknown {
                name: "usr".to_string()
            }
        );
        assert_eq!(
            parse_command("/usr/bin/env python"),
            CommandParse::Unknown {
                name: "usr".to_string()
            }
        );
    }

    #[test]
    fn ends_the_name_at_the_first_non_name_character() {
        let parsed = invocation("/pin!");
        assert_eq!(parsed.name, "pin");
        assert_eq!(parsed.raw_args, "!");
    }

    #[test]
    fn splits_arguments_on_whitespace_and_keeps_raw_text() {
        let parsed = invocation("/task   fix   the bug  ");
        assert_eq!(
            parsed.args,
            vec!["fix".to_string(), "the".to_string(), "bug".to_string()]
        );
        assert_eq!(parsed.raw_args, "fix   the bug");
    }

    #[test]
    fn groups_quoted_arguments_and_strips_the_quotes() {
        let parsed = invocation(r#"/task "write a test" now"#);
        assert_eq!(
            parsed.args,
            vec!["write a test".to_string(), "now".to_string()]
        );
        assert_eq!(parsed.raw_args, r#""write a test" now"#);
    }

    #[test]
    fn keeps_free_text_for_remember_and_note() {
        let parsed = invocation("/remember the deploy lives in staging");
        assert_eq!(parsed.raw_args, "the deploy lives in staging");
        assert_eq!(parsed.args.len(), 5);
    }

    #[test]
    fn accepts_commands_without_arguments() {
        let parsed = invocation("/compact");
        assert_eq!(parsed.name, "compact");
        assert!(parsed.args.is_empty());
        assert_eq!(parsed.raw_args, "");
    }

    #[test]
    fn reports_missing_required_arguments_with_the_catalogue_usage() {
        for (input, name) in [
            ("/rename", "rename"),
            ("/rename   ", "rename"),
            ("/task", "task"),
            ("/remember", "remember"),
            ("/btw", "btw"),
        ] {
            let usage = find_command(name).expect("command exists").usage;
            assert_eq!(
                parse_command(input),
                CommandParse::MissingArgument {
                    name: name.to_string(),
                    usage: usage.to_string(),
                }
            );
        }
    }

    #[test]
    fn quoted_empty_string_satisfies_arity_but_not_value_validation() {
        let parsed = invocation(r#"/rename """#);
        assert_eq!(parsed.args, vec![String::new()]);
    }

    #[test]
    fn catalogue_names_and_aliases_are_unique_and_documented() {
        for spec in command_catalogue() {
            assert!(!spec.name.is_empty(), "a command has no name");
            assert!(
                spec.usage.starts_with(&format!("/{}", spec.name)),
                "{} usage does not start with the command name",
                spec.name
            );
            assert!(!spec.summary.is_empty(), "{} has no summary", spec.name);
            assert!(
                spec.name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'),
                "{} has characters the parser cannot read",
                spec.name
            );

            for alias in spec.aliases {
                assert!(
                    alias
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'),
                    "{alias} has characters the parser cannot read"
                );
                assert!(find_command(alias).is_some_and(|other| other.name == spec.name));
            }
        }

        let mut names: Vec<&str> = command_catalogue().iter().map(|spec| spec.name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "two commands share a name");

        for spec in command_catalogue() {
            for other in command_catalogue() {
                if spec.name == other.name {
                    continue;
                }
                assert!(
                    !other.matches(spec.name),
                    "{} is shadowed by {}",
                    spec.name,
                    other.name
                );
            }
        }
    }

    #[test]
    fn catalogue_is_alphabetical_and_covers_v1() {
        let names: Vec<&str> = command_catalogue().iter().map(|spec| spec.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "the command menu expects alphabetical order");
        assert_eq!(
            sorted,
            vec![
                "approval", "btw", "clear", "compact", "delete", "help", "model", "new", "note",
                "pin", "remember", "rename", "settings", "stop", "task", "unpin",
            ]
        );
    }

    #[test]
    fn choice_arguments_expose_their_values() {
        let approval = find_command("approval").expect("approval exists");
        assert_eq!(approval.args[0].kind.choices(), APPROVAL_CHOICES);
        assert_eq!(approval.target, CommandTarget::Chat);

        let settings = find_command("settings").expect("settings exists");
        assert_eq!(settings.args[0].kind.choices(), SETTINGS_CHOICES);

        let compact = find_command("compact").expect("compact exists");
        assert_eq!(compact.target, CommandTarget::Conversation);
        assert!(compact.args[0].kind.choices().is_empty());
    }

    #[test]
    fn optional_arguments_never_block_an_invocation() {
        for name in ["new", "note", "model", "settings", "clear", "compact"] {
            let spec = find_command(name).expect("command exists");
            assert_eq!(spec.min_args(), 0, "{name} should run without arguments");
            assert!(matches!(
                parse_command(&format!("/{name}")),
                CommandParse::Invocation(_)
            ));
        }
    }

    #[test]
    fn min_args_counts_positional_slots_the_command_cannot_skip() {
        let spec = CommandSpec {
            name: "probe",
            aliases: &[],
            usage: "/probe [@participant] <reason>",
            summary: "A probe with an optional leading argument.",
            args: &[
                CommandArg {
                    placeholder: "@participant",
                    required: false,
                    kind: CommandArgKind::Participant,
                },
                CommandArg {
                    placeholder: "<reason>",
                    required: true,
                    kind: CommandArgKind::Text,
                },
            ],
            target: CommandTarget::Chat,
        };

        // One token fills the optional slot and leaves <reason> empty.
        assert_eq!(spec.min_args(), 2);
        assert_eq!(find_command("rename").expect("rename").min_args(), 1);
        assert_eq!(find_command("pin").expect("pin").min_args(), 0);
    }

    #[test]
    fn serializes_the_whole_catalogue_and_every_parse_variant() {
        let catalogue = serde_json::to_value(command_catalogue())
            .expect("the desktop fetches the catalogue as JSON");
        let approval = &catalogue[0];
        assert_eq!(approval["name"], "approval");
        assert_eq!(approval["target"], "chat");
        assert_eq!(
            approval["args"][0],
            serde_json::json!({
                "placeholder": "safe_auto|ask|accept_edits|plan_only|trusted",
                "required": false,
                "kind": "choice",
                "choices": APPROVAL_CHOICES,
            })
        );

        let compact = catalogue
            .as_array()
            .expect("catalogue is a list")
            .iter()
            .find(|spec| spec["name"] == "compact")
            .expect("compact is catalogued");
        assert_eq!(compact["target"], "conversation");
        assert_eq!(compact["args"][0]["kind"], "participant");
        assert_eq!(compact["args"][0]["choices"], serde_json::json!([]));

        assert_eq!(
            serde_json::to_value(parse_command("hello")).expect("serializes"),
            serde_json::json!({ "kind": "not_a_command" })
        );
        assert_eq!(
            serde_json::to_value(parse_command("/pin")).expect("serializes"),
            serde_json::json!({
                "kind": "invocation",
                "name": "pin",
                "args": [],
                "raw_args": "",
            })
        );
        assert_eq!(
            serde_json::to_value(parse_command("/nope")).expect("serializes"),
            serde_json::json!({ "kind": "unknown", "name": "nope" })
        );
        assert_eq!(
            serde_json::to_value(parse_command("/rename")).expect("serializes"),
            serde_json::json!({
                "kind": "missing_argument",
                "name": "rename",
                "usage": "/rename <name>",
            })
        );
    }

    #[test]
    fn keeps_the_typed_token_when_the_name_grammar_rejects_it() {
        assert_eq!(
            parse_command("/日本語"),
            CommandParse::Unknown {
                name: "日本語".to_string()
            }
        );
        assert_eq!(
            parse_command("/日本語 テキスト"),
            CommandParse::Unknown {
                name: "日本語".to_string()
            }
        );
        assert_eq!(
            parse_command("/.."),
            CommandParse::Unknown {
                name: "..".to_string()
            }
        );
    }
}
