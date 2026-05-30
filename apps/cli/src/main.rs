//! DeepAgent headless demo driver.
//!
//! This binary is not the product UI (that is the Tauri desktop app added in
//! Phase 8). It is a smoke-test driver that wires the Phase 1/2 kernel together
//! and runs a tiny scripted agent end-to-end, proving:
//!
//! - the SQLite database opens & migrates,
//! - a session is created and events are appended,
//! - the runtime loop drives an agent through tool calls,
//! - the session can be recovered purely from the event log,
//! - the Phase 3 context pipeline scans the workspace, injects memory, and
//!   assembles a budgeted five-layer prompt.
//!
//! Run with: `cargo run -p deepagent-cli`

use std::sync::Arc;

use async_trait::async_trait;
use deepagent_context::{ContextPipeline, HeuristicTokenizer, PromptBudget};
use deepagent_core::clock::SystemClock;
use deepagent_core::error::Result;
use deepagent_memory::store::MemoryStore;
use deepagent_memory::{
    to_l5_block, HashingEmbedder, HybridRetriever, MemoryItem, MemoryTier,
    Observation as MemoryObservation, ObservationType,
};
use deepagent_persistence::Database;
use deepagent_runtime::agent::{Agent, AgentDecision, Observation};
use deepagent_runtime::{RuntimeConfig, RuntimeEngine};
use deepagent_session::Session;
use deepagent_tools::permission::{PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolInvocation, ToolOutput, ToolRegistry};
use deepagent_tracing::metrics::Metrics;
use deepagent_workspace::WorkspaceScanner;

/// A trivial tool that reverses a string, to demonstrate tool routing.
struct ReverseTool;

#[async_trait]
impl Tool for ReverseTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "reverse".into(),
            description: "Reverses the characters of `text`.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"]
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
        let text = arguments["text"].as_str().unwrap_or_default();
        let reversed: String = text.chars().rev().collect();
        Ok(ToolOutput::success(
            serde_json::json!({ "reversed": reversed }),
        ))
    }
}

/// A deterministic demo agent: call `reverse` once, then complete.
struct DemoAgent {
    done_tool: bool,
}

#[async_trait]
impl Agent for DemoAgent {
    async fn think(&mut self, _step: usize, last: Option<&Observation>) -> Result<AgentDecision> {
        if !self.done_tool {
            self.done_tool = true;
            return Ok(AgentDecision::CallTool(ToolInvocation::new(
                "reverse",
                serde_json::json!({ "text": "DeepAgent" }),
            )));
        }
        let reversed = last
            .and_then(|o| o.output.get("reversed"))
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        Ok(AgentDecision::Complete(format!(
            "reversed 'DeepAgent' to '{reversed}'"
        )))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    deepagent_tracing::init_dev();

    // Use a temp on-disk DB under the OS temp dir so the demo is self-cleaning
    // across runs but still exercises the real (non in-memory) path.
    let db_path = std::env::temp_dir().join("deepagent-demo.db");
    let _ = std::fs::remove_file(&db_path);
    let db = Database::open(&db_path)?;
    tracing::info!(schema_version = db.schema_version()?, "database ready");

    let clock = SystemClock;

    // Build the tool registry.
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ReverseTool))?;

    let metrics = Metrics::new();
    let config = RuntimeConfig::default();

    // --- Run a session ----------------------------------------------------
    let session_id;
    {
        let mut session = Session::create(&db, &clock, Some("demo session"))?;
        session_id = session.id();
        let task = session.create_task("reverse the product name")?;

        let engine = RuntimeEngine::new(&registry, metrics.clone(), config);
        let mut agent = DemoAgent { done_tool: false };
        let outcome = engine.run(&mut session, task, &mut agent).await?;

        println!("\n=== Run finished ===");
        println!("session : {session_id}");
        println!("outcome : {outcome:?}");
        println!("metrics : {:?}", metrics.snapshot().counters);
    }

    // --- Recover purely from the event log --------------------------------
    let recovered = Session::recover(&db, &clock, session_id)?;
    let state = recovered.state();
    println!("\n=== Recovered from event log ===");
    println!("title           : {:?}", state.title);
    println!("messages        : {}", state.message_count);
    println!("tool calls done : {}", state.tool_calls_completed);
    println!("tasks           :");
    for t in state.tasks() {
        println!("  - [{:?}] {}", t.state, t.goal);
    }

    // --- Phase 3: Context Engineering -------------------------------------
    demo_context_pipeline()?;

    // --- Input dispatch (intent) + Skill system ---------------------------
    demo_intent_and_skills()?;

    // --- Prompt engineering: command/agent loading + system-prompt assembly
    demo_prompts()?;

    Ok(())
}

/// Demonstrates the Phase 3 context engineering stack: scan the current
/// workspace, retrieve relevant memory, and assemble a budgeted five-layer
/// context prompt.
fn demo_context_pipeline() -> Result<()> {
    println!("\n=== Phase 3: Context Engineering ===");

    // L4 — Workspace context: scan the current directory.
    let cwd = std::env::current_dir()
        .map_err(|e| deepagent_core::error::CoreError::other(format!("cannot read cwd: {e}")))?;
    let snapshot = WorkspaceScanner::default().scan(&cwd)?;
    println!(
        "workspace scan  : {} files, {} dirs, kinds={:?}",
        snapshot.file_count, snapshot.dir_count, snapshot.kinds
    );

    // L3 — Memory injection: seed a tiny memory store and retrieve.
    let mut memory = MemoryStore::new();
    let now = deepagent_core::clock::Timestamp::from_millis(1_000);
    memory.insert(MemoryItem::new(
        MemoryTier::Procedural,
        "User prefers Rust and small, well-tested modules.",
        0.9,
        now,
    ));
    memory.insert(MemoryItem::new(
        MemoryTier::Failure,
        "Previously, skipping migrations caused a startup panic.",
        0.8,
        now,
    ));
    let hits = memory.retrieve("rust modules tests", None, 3, now);
    let memory_block = hits
        .iter()
        .map(|h| format!("- {}", h.item.content))
        .collect::<Vec<_>>()
        .join("\n");

    // L5 — Semantic retrieval: hybrid (md + embedding + BM25 + rerank) over
    // markdown observations, the Anthropic / claude-mem retrieval recipe.
    let mut retriever: HybridRetriever<_, _> = HybridRetriever::new(HashingEmbedder::default());
    retriever.insert(
        MemoryObservation::new(ObservationType::BugFix, "Payment timeout fix")
            .narrative("the payment service retries on timeout with exponential backoff")
            .concepts(["payment".into(), "timeout".into()])
            .files(["payment/retry.rs".into()])
            .into_memory_item(now),
    );
    retriever.insert(
        MemoryObservation::new(ObservationType::Feature, "Dashboard charts")
            .narrative("render charts and graphs on the analytics dashboard")
            .concepts(["ui".into()])
            .into_memory_item(now),
    );
    retriever.insert(
        MemoryObservation::new(ObservationType::Knowledge, "Retry budget config key")
            .narrative("RETRY_BUDGET controls the maximum number of retries per request")
            .concepts(["config".into(), "retry".into()])
            .into_memory_item(now),
    );
    let l5_hits = retriever.retrieve("how is payment timeout retry handled", None, 2, now);
    let semantic_block = to_l5_block(&l5_hits);
    println!(
        "L5 hybrid hits  : {} (embedding + BM25 + rerank)",
        l5_hits.len()
    );

    // Assemble the five-layer context, fitted to a budget.
    let tokenizer = HeuristicTokenizer::new();
    let budget = PromptBudget::new(8_000, 1_000, 1_000);
    let outcome = ContextPipeline::new()
        .system_core("You are DeepAgent, a verifiable agent runtime.")
        .safety_rules("Never run destructive commands without approval.")
        .tool_rules("Prefer read tools before write tools.")
        .task_summary("Goal: extend the runtime. Done: core, models, hooks.")
        .workspace(snapshot.to_context_block())
        .memory(memory_block)
        .semantic_retrieval(semantic_block)
        .recent_conversation("user: continue development\nassistant: building Phase 5")
        .user_goal("Wire hybrid semantic retrieval into the context pipeline L5.")
        .compile(&budget, &tokenizer);

    println!(
        "context prompt  : {} tokens, {} fragments kept, {} dropped (allowance {})",
        outcome.prompt.tokens,
        outcome.prompt.fragments.len(),
        outcome.dropped_fragments,
        outcome.allowance
    );
    println!("layers (in order):");
    for frag in &outcome.prompt.fragments {
        println!("  - {:?} (prio {})", frag.source, frag.priority);
    }

    Ok(())
}

/// Demonstrates the input-dispatch layer (slash routing + attachments) and the
/// Skill system (auto-discovery + passive trigger activation + progressive
/// disclosure) end-to-end against the repo's `.deepagent/skills/` tree.
fn demo_intent_and_skills() -> Result<()> {
    use deepagent_intent::{CommandDef, CommandRegistry, Intent, IntentRouter};
    use deepagent_skills::SkillManager;

    println!("\n=== Input Dispatch (Intent) ===");
    let mut commands = CommandRegistry::new();
    commands.register(
        CommandDef::new("review", "Review code for quality")
            .with_body("Review the following for quality:\n$ARGUMENTS")
            .with_allowed_tools(["read_file".into(), "grep".into()]),
    );
    let router = IntentRouter::new(commands);

    for input in [
        "/review src/main.rs for error handling",
        "explain #src/lib.rs to me",
        "/unknown do a thing",
    ] {
        let req = router.route(input);
        match &req.intent {
            Intent::SlashCommand { name, .. } => println!(
                "  /{:<8} → command '{}', allowed_tools={:?}, attachments={}",
                "slash",
                name,
                req.allowed_tools,
                req.attachments.len()
            ),
            Intent::Chat => println!(
                "  {:<9} → chat, attachments={} ({:?})",
                "chat",
                req.attachments.len(),
                req.attachments.iter().map(|a| &a.value).collect::<Vec<_>>()
            ),
            Intent::UnknownCommand { name } => {
                println!("  {:<9} → unknown command '{}'", "unknown", name)
            }
        }
    }

    println!("\n=== Skill System (progressive disclosure) ===");
    // Auto-discover skills from the repo's `.deepagent/skills/` tree.
    let cwd = std::env::current_dir()
        .map_err(|e| deepagent_core::error::CoreError::other(format!("cannot read cwd: {e}")))?;
    let ws_skills = cwd.join(".deepagent").join("skills");
    let install_dir = std::env::temp_dir().join("deepagent-skills-installed");

    let mut skills = SkillManager::new(Some(ws_skills), install_dir);
    let count = skills.load_all()?;
    println!("discovered      : {count} skill(s)");
    for meta in skills.registry().catalog() {
        println!(
            "  - {} [{}] triggers: {}",
            meta.id,
            meta.origin.label(),
            skills
                .registry()
                .get(&meta.id)
                .map(|s| s.triggers.len())
                .unwrap_or(0)
        );
    }

    // Passive activation: a user query is matched against trigger phrases and
    // the best skill's body is disclosed (Level 1 → Level 2).
    let query = "can you review rust code in this crate for error handling?";
    match skills.auto_activate(query) {
        Some((id, fragment)) => {
            println!("passive match   : query → skill '{id}'");
            let preview: String = fragment.content.chars().take(80).collect();
            println!("disclosed body  : {}…", preview.replace('\n', " "));
        }
        None => println!("passive match   : (none)"),
    }

    Ok(())
}

/// Demonstrates prompt engineering: load command/agent definitions from
/// `.deepagent/` and assemble a Claude-Code-structured system prompt over the
/// context Prompt AST.
fn demo_prompts() -> Result<()> {
    use deepagent_context::HeuristicTokenizer;
    use deepagent_prompts::{discover_commands, AgentDef, SystemPromptBuilder};

    println!("\n=== Prompt Engineering (commands / agents / system prompt) ===");

    let cwd = std::env::current_dir()
        .map_err(|e| deepagent_core::error::CoreError::other(format!("cannot read cwd: {e}")))?;
    let deepagent = cwd.join(".deepagent");

    // Load slash commands from `.deepagent/commands/`.
    let commands = discover_commands(deepagent.join("commands"))?;
    println!("commands loaded : {}", commands.len());
    for c in &commands {
        println!(
            "  - /{} — {} (allowed_tools={:?})",
            c.name, c.description, c.allowed_tools
        );
    }

    // Load an agent definition from `.deepagent/agents/`.
    let agent_path = deepagent.join("agents").join("rust-architect.md");
    let agent = std::fs::read_to_string(&agent_path)
        .ok()
        .and_then(|raw| AgentDef::parse(&raw));

    // Assemble a layered system prompt (optionally adopting the agent persona).
    let counter = HeuristicTokenizer::new();
    let mut builder = SystemPromptBuilder::new()
        .core("You are DeepAgent, a verifiable Rust-native agent runtime.")
        .safety("Never run destructive commands without explicit approval.")
        .workspace_rule("Match the crate's existing conventions; keep modules small and tested.")
        .tool_rule("Prefer read tools before write tools.")
        .user_goal("Design the prompt-assembly layer.");

    if let Some(agent) = &agent {
        println!(
            "agent loaded    : {} (model={:?}, tools={})",
            agent.name,
            agent.model,
            agent.tools.len()
        );
        builder = builder.with_agent(agent);
    }

    let compiled = builder.compile(&counter);
    println!(
        "system prompt   : {} tokens, {} layers",
        compiled.tokens,
        compiled.fragments.len()
    );
    println!("layers (in order):");
    for frag in &compiled.fragments {
        println!("  - {:?} (prio {})", frag.source, frag.priority);
    }

    Ok(())
}
