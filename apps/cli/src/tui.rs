use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use deepagent_app_core::{ApprovalRequestDto, ChatService, HarnessRunOverrides};
use deepagent_harness_protocol::EventContext;
use deepagent_runtime::RuntimeEvent;

use crate::args::RunOptions;

const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";
const RESET: &str = "\x1b[0m";
const ACCENT: &str = "\x1b[38;5;209m";
const MUTED: &str = "\x1b[38;5;244m";
const DIM: &str = "\x1b[2m";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    Empty,
    Submit(String),
    Help,
    Clear,
    Status,
    Resume(String),
    Quit,
}

pub fn parse_input(raw: &str) -> InputAction {
    let input = raw.trim();
    if input.is_empty() {
        return InputAction::Empty;
    }
    if matches!(input, "/quit" | "/exit" | "/q") {
        return InputAction::Quit;
    }
    if input == "/help" {
        return InputAction::Help;
    }
    if input == "/clear" {
        return InputAction::Clear;
    }
    if input == "/status" {
        return InputAction::Status;
    }
    if let Some(thread_id) = input.strip_prefix("/resume ") {
        let thread_id = thread_id.trim();
        if !thread_id.is_empty() {
            return InputAction::Resume(thread_id.to_string());
        }
    }
    InputAction::Submit(input.to_string())
}

pub fn welcome_screen(workspace: &Path, model: Option<&str>, sandbox: Option<&str>) -> String {
    let model = model.unwrap_or("configured model");
    let sandbox = sandbox.unwrap_or("direct");
    format!(
        "{CLEAR_SCREEN}{ACCENT}DeepAgent Studio{RESET}  {DIM}interactive CLI{RESET}\r\n\
         {ACCENT}+----------------------+  +----------------------------------+{RESET}\r\n\
         {ACCENT}|{RESET} {ACCENT}Welcome back!{RESET}     {ACCENT}|  |{RESET} {ACCENT}Tips for getting started{RESET}      {ACCENT}|{RESET}\r\n\
         {ACCENT}|{RESET}                      {ACCENT}|  |{RESET} Type a task and press Enter.    {ACCENT}|{RESET}\r\n\
         {ACCENT}|{RESET} {MUTED}DeepAgent Studio{RESET}   {ACCENT}|  |{RESET} Use /help for commands.         {ACCENT}|{RESET}\r\n\
         {ACCENT}|{RESET} {MUTED}{workspace:<20}{RESET} {ACCENT}|  |{RESET} /status shows the active thread. {ACCENT}|{RESET}\r\n\
         {ACCENT}|{RESET} {MUTED}{model:<20}{RESET} {ACCENT}|  |{RESET} /resume <id> continues a thread. {ACCENT}|{RESET}\r\n\
         {ACCENT}|{RESET} {MUTED}{sandbox:<20}{RESET} {ACCENT}|  |{RESET} /quit exits the terminal UI.     {ACCENT}|{RESET}\r\n\
         {ACCENT}+----------------------+  +----------------------------------+{RESET}\r\n\
         {MUTED}manual mode on  |  /help for shortcuts{RESET}\r\n\
         {MUTED}----------------------------------------------------------------{RESET}\r\n\
         > ",
        workspace = truncate(&compact_path(workspace, 20), 20),
        model = truncate(model, 20),
        sandbox = truncate(sandbox, 20),
    )
}

pub async fn run(mut options: RunOptions) -> Result<(), String> {
    let workspace =
        std::env::current_dir().map_err(|error| format!("read current directory: {error}"))?;
    let (chat, backend_capabilities) = super::build_chat_service(&workspace, &options)?;
    if let Some(capabilities) = backend_capabilities.as_ref() {
        if !capabilities.available {
            return Err(capabilities.message.clone());
        }
    }

    print!(
        "{}",
        welcome_screen(
            &workspace,
            options.model.as_deref(),
            options.sandbox_backend.as_deref(),
        )
    );
    io::stdout()
        .flush()
        .map_err(|error| format!("flush terminal: {error}"))?;

    let mut thread_id = options.continue_thread.take();
    let stdin = io::stdin();
    loop {
        let mut line = String::new();
        if stdin
            .read_line(&mut line)
            .map_err(|error| format!("read terminal input: {error}"))?
            == 0
        {
            println!();
            return Ok(());
        }

        match parse_input(&line) {
            InputAction::Empty => {
                print!("> ");
            }
            InputAction::Quit => {
                println!("{MUTED}Goodbye.{RESET}");
                return Ok(());
            }
            InputAction::Help => print_help(),
            InputAction::Clear => {
                print!(
                    "{}",
                    welcome_screen(
                        &workspace,
                        options.model.as_deref(),
                        options.sandbox_backend.as_deref(),
                    )
                );
            }
            InputAction::Status => print_status(
                &workspace,
                thread_id.as_deref(),
                backend_capabilities.as_ref(),
            ),
            InputAction::Resume(id) => {
                thread_id = Some(id.clone());
                println!("{MUTED}Resuming thread {id}.{RESET}");
                print!("> ");
            }
            InputAction::Submit(prompt) => {
                let result = run_turn(&chat, &options, &prompt, thread_id.as_deref()).await;
                match result {
                    Ok(id) => {
                        thread_id = Some(id);
                        println!("\r\n{MUTED}turn complete{RESET}\r\n");
                    }
                    Err(error) => {
                        println!("\r\n{ACCENT}error:{RESET} {error}\r\n");
                    }
                }
                print!("> ");
            }
        }
        io::stdout()
            .flush()
            .map_err(|error| format!("flush terminal: {error}"))?;
    }
}

async fn run_turn(
    chat: &ChatService,
    options: &RunOptions,
    prompt: &str,
    thread_id: Option<&str>,
) -> Result<String, String> {
    println!();
    let context = Arc::new(Mutex::new(EventContext::default()));
    let context_for_events = context.clone();
    let on_event = move |event: RuntimeEvent| {
        render_event(&event, &context_for_events);
    };

    let context_for_approval = context.clone();
    let chat_for_approval = chat.clone();
    let on_approval = move |approval: ApprovalRequestDto| {
        let context = context_for_approval
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let thread_id = context.thread_id.as_deref().unwrap_or("current thread");
        println!(
            "\r\n{ACCENT}approval required{RESET} [{thread_id}] {}: {}",
            approval.tool, approval.reason
        );
        print!("Approve? [y/N] ");
        let _ = io::stdout().flush();
        let approved = super::read_approval_decision();
        if let Err(error) =
            chat_for_approval.resolve_approval(&approval.call_id, approved, "cli_tui_user")
        {
            eprintln!(
                "failed to persist approval response for {}: {error}",
                approval.call_id
            );
        }
    };

    let overrides = HarnessRunOverrides {
        provider: options.provider.clone(),
        model: options.model.clone(),
        reasoning_effort: options.reasoning_effort.clone(),
        sandbox_backend: options.sandbox_backend.clone(),
        permission_profile: options.permission_profile.clone(),
    };

    chat.run_in_session_with_overrides(
        prompt,
        thread_id,
        None,
        None,
        Vec::new(),
        None,
        false,
        None,
        overrides,
        on_event,
        on_approval,
    )
    .await
    .map_err(|error| error.to_string())
}

fn render_event(event: &RuntimeEvent, context: &Arc<Mutex<EventContext>>) {
    {
        let mut guard = context
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match event {
            RuntimeEvent::SessionRegistered { session_id, .. } => {
                guard.thread_id = Some(session_id.clone());
            }
            RuntimeEvent::RunStarted { task_id } => {
                guard.turn_id = Some(task_id.clone());
            }
            _ => {}
        }
    }

    match event {
        RuntimeEvent::ContentDelta { text } => print!("{text}"),
        RuntimeEvent::ReasoningDelta { .. } => {}
        RuntimeEvent::ToolStarted { name, summary, .. } => {
            let summary = summary.as_deref().unwrap_or("running");
            print!("\r\n{MUTED}* {name}: {summary}{RESET}\r\n");
        }
        RuntimeEvent::ToolCompleted { name, ok, .. } => {
            let state = if *ok { "done" } else { "failed" };
            print!("{MUTED}  {name}: {state}{RESET}\r\n");
        }
        RuntimeEvent::ToolBlocked {
            name,
            reason,
            needs_approval,
        } => {
            let state = if *needs_approval {
                "awaiting approval"
            } else {
                "blocked"
            };
            print!("{ACCENT}! {name}: {state} ({reason}){RESET}\r\n");
        }
        RuntimeEvent::RunFailed { reason } => {
            print!("\r\n{ACCENT}run failed:{RESET} {reason}\r\n");
        }
        RuntimeEvent::RunCancelled => {
            print!("\r\n{MUTED}run interrupted{RESET}\r\n");
        }
        RuntimeEvent::RunCompleted { .. } => {
            print!("\r\n");
        }
        _ => {}
    }
    let _ = io::stdout().flush();
}

fn print_help() {
    println!(
        "\r\n{ACCENT}Commands{RESET}\r\n\
         /help              show this help\r\n\
         /clear             clear the screen\r\n\
         /status            show current thread and workspace\r\n\
         /resume <thread>   continue an existing thread\r\n\
         /quit              exit DeepAgent\r\n"
    );
    print!("> ");
}

fn print_status(
    workspace: &Path,
    thread_id: Option<&str>,
    capabilities: Option<&deepagent_app_core::SandboxCapabilities>,
) {
    let sandbox = capabilities
        .map(|capability| format!("{:?}", capability.kind).to_lowercase())
        .unwrap_or_else(|| "unknown".to_string());
    println!(
        "\r\n{ACCENT}Status{RESET}\r\n\
         workspace: {}\r\n\
         thread:    {}\r\n\
         sandbox:   {}\r\n",
        workspace.display(),
        thread_id.unwrap_or("not started"),
        sandbox
    );
    print!("> ");
}

fn compact_path(path: &Path, width: usize) -> String {
    truncate(&path.display().to_string(), width)
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    let prefix: String = value.chars().take(width.saturating_sub(3)).collect();
    format!("{prefix}...")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_interactive_commands() {
        assert_eq!(parse_input("/help"), InputAction::Help);
        assert_eq!(parse_input("/clear"), InputAction::Clear);
        assert_eq!(parse_input("/status"), InputAction::Status);
        assert_eq!(
            parse_input("/resume thread-1"),
            InputAction::Resume("thread-1".into())
        );
        assert_eq!(parse_input("/quit"), InputAction::Quit);
    }

    #[test]
    fn treats_regular_text_as_a_prompt() {
        assert_eq!(
            parse_input("  inspect the repository  "),
            InputAction::Submit("inspect the repository".into())
        );
    }

    #[test]
    fn welcome_screen_contains_workspace_and_prompt() {
        let screen = welcome_screen(Path::new("C:\\repo"), Some("deepseek-chat"), Some("direct"));
        assert!(screen.contains("DeepAgent Studio"));
        assert!(screen.contains("C:\\repo"));
        assert!(screen.contains("deepseek-chat"));
        assert!(screen.contains("> "));
    }
}
