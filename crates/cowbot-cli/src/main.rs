mod config;

use clap::{Parser, Subcommand, ValueEnum};
use cowbot_agent::{AgentInstance, Crew, StandardRoles};
use cowbot_client::{Client, LocalClient};
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::channel::{ChannelId, ChannelMessage, Sender};
use cowbot_core::event::EventPayload;
use cowbot_core::task::Task;
use cowbot_llm::{LlmProvider, MockLlmProvider, OpenAiCompatibleProvider};
use cowbot_service::{ChannelTurnOptions, CrewRunOptions};
use cowbot_store::Store;
use cowbot_tools::{
    CodeExecutionTool, ReadFileTool, SandboxBackend, SubprocessSandbox, ToolRegistry, WriteFileTool,
};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader, Lines};
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

use config::{FileConfig, Overrides, Settings};

#[derive(Parser)]
#[command(name = "cowbot")]
#[command(author = "CowBot Contributors")]
#[command(version = "0.1.4")]
#[command(about = "CowBot: High-performance Multi-Agent Collaboration Platform in Rust", long_about = None)]
struct Cli {
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Path to the configuration file (defaults to ./cowbot.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize the local database and workspace
    Init {
        /// SQLite database path
        #[arg(long, env = "COWBOT_DB")]
        db: Option<PathBuf>,
    },

    /// Run an autonomous multi-agent crew on an engineering task
    Run {
        /// The objective or task prompt to execute
        #[arg(short, long)]
        task: String,

        /// Model name to use (e.g. gpt-4o, deepseek-chat, llama3)
        #[arg(short, long)]
        model: Option<String>,

        /// Custom OpenAI-compatible base URL (e.g. http://localhost:11434/v1 for Ollama)
        #[arg(long, env = "OPENAI_BASE_URL")]
        base_url: Option<String>,

        /// API key for the LLM provider
        #[arg(long, env = "OPENAI_API_KEY")]
        api_key: Option<String>,

        /// Use mock LLM provider for local dry-run testing
        #[arg(long)]
        dry_run: bool,

        /// SQLite database path
        #[arg(long, env = "COWBOT_DB")]
        db: Option<PathBuf>,

        /// Approval policy for tool calls
        #[arg(long, value_enum)]
        approval: Option<ApprovalLevelArg>,

        /// Maximum number of assignments to execute concurrently
        #[arg(long)]
        concurrency: Option<usize>,

        /// Post the crew's tasks into this channel
        #[arg(long)]
        channel: Option<String>,
    },

    /// Inspect registered tools and test sandbox code execution
    Tools,

    /// Export the local workspace to a `.cowpkg` file
    Export {
        /// Destination path for the package
        #[arg(short, long)]
        output: PathBuf,

        /// SQLite database path
        #[arg(long, env = "COWBOT_DB")]
        db: Option<PathBuf>,
    },

    /// Import a `.cowpkg` file as a new workspace
    Import {
        /// Path to the package to import
        #[arg(short, long)]
        input: PathBuf,

        /// SQLite database path
        #[arg(long, env = "COWBOT_DB")]
        db: Option<PathBuf>,
    },

    /// List or create collaboration channels
    Channel {
        #[command(subcommand)]
        command: ChannelCommands,
    },

    /// List or post channel messages
    Message {
        #[command(subcommand)]
        command: MessageCommands,
    },
}

#[derive(Subcommand)]
enum ChannelCommands {
    /// List channels in the local workspace
    List,
    /// Create a channel
    Create {
        /// Channel name
        #[arg(long)]
        name: String,
        /// Optional channel topic
        #[arg(long, default_value = "")]
        topic: String,
    },
}

#[derive(Subcommand)]
enum MessageCommands {
    /// List messages in a channel
    List {
        /// Channel id
        #[arg(long)]
        channel: String,
        /// Maximum number of messages to return
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Post a message to a channel
    Post {
        /// Channel id
        #[arg(long)]
        channel: String,
        /// Message content
        #[arg(long)]
        content: String,
        /// Optional thread id
        #[arg(long)]
        thread: Option<String>,
        /// Let agents mentioned in the message answer before exiting
        #[arg(long)]
        wait: bool,
        /// API key for the mentioned agents' model (defaults to OPENAI_API_KEY)
        #[arg(long, env = "OPENAI_API_KEY", hide_env_values = true)]
        api_key: Option<String>,
        /// Run mentioned agents against the offline mock model
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ApprovalLevelArg {
    Ask,
    SafeAuto,
    PlanOnly,
    Trusted,
}

impl From<ApprovalLevelArg> for ApprovalLevel {
    fn from(value: ApprovalLevelArg) -> Self {
        match value {
            ApprovalLevelArg::Ask => ApprovalLevel::Ask,
            ApprovalLevelArg::SafeAuto => ApprovalLevel::SafeAuto,
            ApprovalLevelArg::PlanOnly => ApprovalLevel::PlanOnly,
            ApprovalLevelArg::Trusted => ApprovalLevel::Trusted,
        }
    }
}

async fn export_package(output: PathBuf, settings: &Settings) -> anyhow::Result<()> {
    let store = Store::connect(&settings.db).await?;
    store.migrate().await?;
    let context = cowbot_service::bootstrap_local(&store).await?;

    let bytes = cowbot_package::export_workspace(&store, &context.workspace.id).await?;
    std::fs::write(&output, &bytes)?;

    println!("Exported workspace to {}", output.display());
    println!("  bytes:     {}", bytes.len());
    println!("  workspace: {}", context.workspace.name);
    Ok(())
}

async fn import_package(input: PathBuf, settings: &Settings) -> anyhow::Result<()> {
    let bytes = std::fs::read(&input)?;

    let manifest = cowbot_package::inspect(&bytes)?;
    let store = Store::connect(&settings.db).await?;
    store.migrate().await?;
    let summary = cowbot_package::import_workspace(&store, &bytes).await?;

    println!("Imported package from {}", input.display());
    println!(
        "  workspace:    {} ({})",
        summary.workspace_name, summary.workspace_id
    );
    println!("  exported by:  {}", manifest.exported_by.workspace_name);
    println!(
        "  contents:     {} channels, {} messages, {} agents, {} tasks, {} runs",
        summary.channels, summary.messages, summary.agents, summary.tasks, summary.runs
    );
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let log_level = if cli.verbose {
        Level::DEBUG
    } else {
        Level::INFO
    };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_target(false)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);

    let file: FileConfig = config::load(cli.config.as_deref())?;

    match cli.command {
        Commands::Init { db } => {
            let settings = config::resolve(
                Overrides {
                    db,
                    ..Overrides::default()
                },
                file,
            );
            initialize(&settings).await?;
        }
        Commands::Run {
            task,
            model,
            base_url,
            api_key,
            dry_run,
            db,
            approval,
            concurrency,
            channel,
        } => {
            let settings = config::resolve(
                Overrides {
                    db,
                    model,
                    base_url,
                    approval,
                    concurrency,
                },
                file,
            );
            run_crew(task, settings, api_key, dry_run, channel).await?;
        }
        Commands::Tools => {
            inspect_tools().await?;
        }
        Commands::Export { output, db } => {
            let settings = config::resolve(
                Overrides {
                    db,
                    ..Overrides::default()
                },
                file,
            );
            export_package(output, &settings).await?;
        }
        Commands::Import { input, db } => {
            let settings = config::resolve(
                Overrides {
                    db,
                    ..Overrides::default()
                },
                file,
            );
            import_package(input, &settings).await?;
        }
        Commands::Channel { command } => {
            let settings = config::resolve(Overrides::default(), file);
            handle_channel(command, &settings).await?;
        }
        Commands::Message { command } => {
            let settings = config::resolve(Overrides::default(), file);
            handle_message(command, &settings).await?;
        }
    }

    Ok(())
}

async fn initialize(settings: &Settings) -> anyhow::Result<()> {
    let client = LocalClient::open(&settings.db).await?;
    let session = client.session().await?;

    println!("CowBot local workspace ready");
    println!("  database:  {}", settings.db.display());
    println!(
        "  owner:     {} <{}>",
        session.user.name, session.user.email
    );
    println!(
        "  workspace: {} ({})",
        session.workspace.name, session.workspace.id
    );

    Ok(())
}

/// Validate a channel id against the local workspace.
async fn require_channel(client: &LocalClient, channel: &str) -> anyhow::Result<ChannelId> {
    let channel_id = ChannelId::new(channel);
    let channels = client.list_channels().await?;
    if !channels.iter().any(|known| known.id == channel_id) {
        anyhow::bail!("channel '{channel_id}' was not found");
    }
    Ok(channel_id)
}

async fn handle_channel(command: ChannelCommands, settings: &Settings) -> anyhow::Result<()> {
    let client = LocalClient::open(&settings.db).await?;
    match command {
        ChannelCommands::List => list_channels(&client).await?,
        ChannelCommands::Create { name, topic } => {
            create_channel(&client, &name, &topic).await?;
        }
    }
    Ok(())
}

async fn list_channels(client: &LocalClient) -> anyhow::Result<()> {
    let channels = client.list_channels().await?;
    if channels.is_empty() {
        println!("No channels yet. Create one with `cowbot channel create --name <name>`.");
        return Ok(());
    }

    println!("Channels:");
    for channel in channels {
        if channel.topic.trim().is_empty() {
            println!("  {}  {}", channel.id, channel.name);
        } else {
            println!("  {}  {} — {}", channel.id, channel.name, channel.topic);
        }
    }
    Ok(())
}

async fn create_channel(client: &LocalClient, name: &str, topic: &str) -> anyhow::Result<()> {
    let channel = client.create_channel(name, topic).await?;
    println!("Created channel {} ({}).", channel.id, channel.name);
    Ok(())
}

async fn handle_message(command: MessageCommands, settings: &Settings) -> anyhow::Result<()> {
    let client = LocalClient::open(&settings.db).await?;
    match command {
        MessageCommands::List { channel, limit } => {
            list_messages(&client, &channel, limit).await?;
        }
        MessageCommands::Post {
            channel,
            content,
            thread,
            wait,
            api_key,
            dry_run,
        } => {
            let message = post_message(&client, &channel, &content, thread.as_deref()).await?;
            if wait {
                reply_to_mentions(&client, &message, settings, api_key, dry_run).await?;
            }
        }
    }
    Ok(())
}

/// Run one channel turn for the agents a message mentions.
async fn reply_to_mentions(
    client: &LocalClient,
    message: &ChannelMessage,
    settings: &Settings,
    api_key: Option<String>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let sandbox = Arc::new(SubprocessSandbox::default());
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(CodeExecutionTool::new(sandbox)));
    tools.register(Arc::new(ReadFileTool::new(None)));
    tools.register(Arc::new(WriteFileTool::new(None)));

    let llm: Arc<dyn LlmProvider> = if dry_run || (api_key.is_none() && settings.base_url.is_none())
    {
        println!("Note: replying with the offline MockLlmProvider.");
        Arc::new(MockLlmProvider::echo())
    } else {
        let url = settings
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        Arc::new(OpenAiCompatibleProvider::new(url, api_key))
    };

    let report = client
        .reply_to_mentions(
            &message.channel_id,
            message,
            llm,
            tools,
            &settings.model,
            ChannelTurnOptions {
                approval_level: settings.approval.into(),
                max_concurrency: settings.concurrency,
            },
        )
        .await?;

    if report.is_empty() {
        println!("No agent was mentioned; nothing to answer.");
        return Ok(());
    }
    println!(
        "{} agent(s) answered in channel {}.",
        report.agents.len(),
        message.channel_id
    );
    for name in &report.unknown_mentions {
        println!("  no agent matched @{name}");
    }

    list_messages(client, &message.channel_id.0, 20).await?;
    Ok(())
}

fn sender_label(sender: &Sender) -> String {
    match sender {
        Sender::User(id) => format!("user:{id}"),
        Sender::Agent(id) => format!("agent:{id}"),
        Sender::System => "system".to_string(),
    }
}

async fn list_messages(client: &LocalClient, channel: &str, limit: u32) -> anyhow::Result<()> {
    let channel_id = ChannelId::new(channel);
    let messages = client.list_messages(&channel_id, limit).await?;
    if messages.is_empty() {
        println!("No messages in channel {channel_id}.");
        return Ok(());
    }

    for message in messages {
        let thread = message
            .thread_id
            .as_deref()
            .map(|thread| format!(" (thread {thread})"))
            .unwrap_or_default();
        println!(
            "[{}] {}{}: {}",
            message.created_at.to_rfc3339(),
            sender_label(&message.sender),
            thread,
            message.content
        );
    }
    Ok(())
}

async fn post_message(
    client: &LocalClient,
    channel: &str,
    content: &str,
    thread: Option<&str>,
) -> anyhow::Result<ChannelMessage> {
    let channel_id = require_channel(client, channel).await?;
    let message = match thread {
        Some(thread) => {
            client
                .post_thread_message(&channel_id, content, thread)
                .await?
        }
        None => client.post_message(&channel_id, content).await?,
    };
    println!("Posted message {} to channel {}.", message.id, channel_id);
    Ok(message)
}

async fn run_crew(
    task_prompt: String,
    settings: Settings,
    api_key: Option<String>,
    dry_run: bool,
    channel: Option<String>,
) -> anyhow::Result<()> {
    let client = Arc::new(LocalClient::open(&settings.db).await?);
    let session = client.session().await?;
    let channel_id = match channel {
        Some(channel) => Some(require_channel(&client, &channel).await?),
        None => None,
    };

    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║       🐄 CowBot Multi-Agent Collaboration Platform       ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");
    println!("Objective: {}\n", task_prompt);
    println!("Database:  {}", settings.db.display());
    println!("Workspace: {}\n", session.workspace.name);

    let mut rx = client.subscribe();
    let approval_client = Arc::clone(&client);

    // Print live events and prompt for approvals while the crew runs.
    let listener = tokio::spawn(async move {
        let mut stdin = BufReader::new(tokio::io::stdin()).lines();

        while let Some(event) = rx.recv().await {
            match event.payload {
                EventPayload::TaskCreated(t) => {
                    println!("[TASK CREATED] {}", t.description);
                }
                EventPayload::TaskAssigned {
                    task_id: _,
                    agent_id,
                } => {
                    println!("[ASSIGNED] -> Agent '{}'", agent_id);
                }
                EventPayload::AgentThought { agent_id, thought } => {
                    println!("\n💭 [{}] Thought:\n{}", agent_id, thought.trim());
                }
                EventPayload::ToolCallRequested { agent_id, call } => {
                    println!(
                        "\n🔧 [{}] Tool Call: {} (args: {})",
                        agent_id, call.name, call.arguments
                    );
                }
                EventPayload::ToolCallExecuted { agent_id, result } => {
                    let status = if result.success { "SUCCESS" } else { "FAILED" };
                    println!(
                        "⚡ [{}] Tool Result [{}]:\n{}",
                        agent_id,
                        status,
                        result.output.trim()
                    );
                }
                EventPayload::TaskCompleted { task_id: _, output } => {
                    println!("\n✅ [TASK COMPLETED]\n{}\n", output.trim());
                }
                EventPayload::TaskFailed { task_id: _, error } => {
                    println!("\n❌ [TASK FAILED]: {}", error);
                }
                EventPayload::ApprovalRequested {
                    approval,
                    risk,
                    reason,
                } => {
                    println!("\n🔐 Approval required: {}", reason);
                    println!(
                        "   risk: {}, tool call: {}, task: {}",
                        risk.as_str(),
                        approval.tool_call_id,
                        approval.task_id
                    );
                    let approved = prompt_approval(&mut stdin).await;
                    match approval_client
                        .decide_approval(&approval.id, approved)
                        .await
                    {
                        Ok(_) => println!(
                            "   decision: {}\n",
                            if approved { "approved" } else { "rejected" }
                        ),
                        Err(err) => eprintln!("Failed to record approval decision: {err}"),
                    }
                }
                _ => {}
            }
        }
    });

    // Initialize Sandbox & Tools
    let sandbox = Arc::new(SubprocessSandbox::default());
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(CodeExecutionTool::new(sandbox)));
    tools.register(Arc::new(ReadFileTool::new(None)));
    tools.register(Arc::new(WriteFileTool::new(None)));

    // Initialize LLM Provider
    let llm: Arc<dyn LlmProvider> = if dry_run || (api_key.is_none() && settings.base_url.is_none())
    {
        println!("Note: Running in dry-run mode using MockLlmProvider.");
        println!("      Set OPENAI_API_KEY or --base-url to use a live LLM endpoint.\n");
        Arc::new(MockLlmProvider::echo())
    } else {
        let url = settings
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        Arc::new(OpenAiCompatibleProvider::new(url, api_key))
    };

    // Assemble Crew (Planner + Coder + Reviewer)
    let planner_profile = StandardRoles::planner();
    let planner_id = planner_profile.id.clone();
    let planner = AgentInstance::new(planner_profile, llm.clone(), tools.clone(), &settings.model);

    let coder_profile = StandardRoles::coder();
    let coder_id = coder_profile.id.clone();
    let coder = AgentInstance::new(coder_profile, llm.clone(), tools.clone(), &settings.model);

    let reviewer_profile = StandardRoles::reviewer();
    let reviewer_id = reviewer_profile.id.clone();
    let reviewer = AgentInstance::new(
        reviewer_profile,
        llm.clone(),
        tools.clone(),
        &settings.model,
    );

    let mut crew = Crew::new();
    crew.add_agent(planner);
    crew.add_agent(coder);
    crew.add_agent(reviewer);

    // Build Task Pipeline
    let plan_task = Task::new(
        format!(
            "Analyze objective and formulate architecture plan: {}",
            task_prompt
        ),
        "A clear, step-by-step implementation and verification plan.",
    )
    .assign_to(planner_id);
    let plan_id = plan_task.id.clone();

    let code_task = Task::new(
        "Execute code implementation and verify functionality in the sandbox.",
        "Working source code and sandbox execution results.",
    )
    .assign_to(coder_id)
    .with_context_task(plan_id.clone());
    let code_id = code_task.id.clone();

    let review_task = Task::new(
        "Review implementation, tests, and diffs for correctness and edge cases.",
        "Comprehensive review findings and final approval status.",
    )
    .assign_to(reviewer_id)
    .with_context_task(code_id);

    crew.add_task(plan_task);
    crew.add_task(code_task);
    crew.add_task(review_task);

    // Execute through the local client so tasks, assignments, runs, tool
    // calls, approvals, and audit records are persisted.
    let results = client
        .run_crew_with_options(
            &mut crew,
            CrewRunOptions {
                approval_level: settings.approval.into(),
                channel_id,
                max_concurrency: settings.concurrency,
            },
        )
        .await?;
    listener.abort();

    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║                    Execution Summary                         ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    for (task_id, output) in results {
        println!("\nTask ID: {}\nDeliverable:\n{}", task_id, output);
    }
    println!("\nHistory persisted to {}.", settings.db.display());

    Ok(())
}

async fn prompt_approval<R>(lines: &mut Lines<R>) -> bool
where
    R: AsyncBufRead + Unpin,
{
    loop {
        println!("Approve this tool call? [y/N]");
        match lines.next_line().await {
            Ok(Some(line)) => match line.trim().to_lowercase().as_str() {
                "y" | "yes" => return true,
                "n" | "no" | "" => return false,
                _ => println!("Please answer y or n."),
            },
            _ => return false,
        }
    }
}

async fn inspect_tools() -> anyhow::Result<()> {
    let sandbox = Arc::new(SubprocessSandbox::default());
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(CodeExecutionTool::new(sandbox.clone())));
    registry.register(Arc::new(ReadFileTool::new(None)));
    registry.register(Arc::new(WriteFileTool::new(None)));

    println!("\nAvailable CowBot Tools:\n");
    for def in registry.definitions() {
        println!("* {} - {}", def.name, def.description);
        println!("  Schema: {}\n", def.parameters);
    }

    println!("Testing Subprocess Sandbox execution...");
    let cmd = "echo Sandbox Active";
    let res = sandbox.execute(cmd, None, 5).await?;
    println!("Sandbox Test Exit Code: {:?}", res.exit_code);
    println!("Sandbox Test Output: {}", res.stdout.trim());

    Ok(())
}
