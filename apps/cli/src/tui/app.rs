use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use deepagent_app_core::dto::{ApprovalRequestDto, QuestionRequestDto};
use deepagent_app_core::{ChatService, HarnessRunOverrides};
use deepagent_builtins::Question;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::args::RunOptions;
use crate::tui::event::{self, TuiEvent};
use crate::tui::history::{display_width, truncate, HistoryCell};
use crate::tui::host::{self, UiEvent};
use crate::tui::questions::join_selection;
use crate::tui::state::{ChatState, Continue};
use crate::tui::terminal::TerminalGuard;

/// Rows owned by the live viewport: status, streaming tail, composer/overlay,
/// hint. ratatui 0.30 has no `set_viewport`, so the height stays fixed.
const VIEWPORT_ROWS: u16 = 4;
const SPINNER: [char; 4] = ['|', '/', '-', '\\'];
const SPINNER_PERIOD: Duration = Duration::from_millis(120);

pub async fn run(options: RunOptions) -> Result<(), String> {
    let workspace =
        std::env::current_dir().map_err(|error| format!("read current directory: {error}"))?;
    let (chat, capabilities) = crate::build_chat_service(&workspace, &options)?;
    if let Some(capabilities) = capabilities.as_ref().filter(|caps| !caps.available) {
        return Err(capabilities.message.clone());
    }

    let (ui_sender, mut ui_receiver) = tokio::sync::mpsc::unbounded_channel::<UiEvent>();
    let chat = host::attach_question_channel(chat, ui_sender.clone());
    let mut app = App::new(workspace, &options, chat, ui_sender);
    let mut guard = TerminalGuard::inline(VIEWPORT_ROWS)?;

    let (input_sender, mut input_receiver) = tokio::sync::mpsc::channel::<TuiEvent>(64);
    let pump = event::spawn_pump(input_sender);
    let mut ticker = tokio::time::interval(SPINNER_PERIOD);

    app.collect_flushable();
    paint(&mut guard, &app)?;
    let mut outcome = Ok(());
    loop {
        let quit = tokio::select! {
            input = input_receiver.recv() => match input {
                Some(event) => app.on_tui_event(event),
                None => true,
            },
            ui = <UnboundedReceiver<UiEvent>>::recv(&mut ui_receiver) => match ui {
                Some(event) => {
                    app.on_ui_event(event);
                    false
                }
                None => true,
            },
            _ = ticker.tick() => app.on_tick(),
        };
        if app.dirty {
            app.collect_flushable();
            let cells = app.take_flushable();
            if let Err(error) = flush(&mut guard, &cells) {
                outcome = Err(error);
                break;
            }
            if let Err(error) = paint(&mut guard, &app) {
                outcome = Err(error);
                break;
            }
            app.dirty = false;
        }
        if quit {
            break;
        }
    }
    pump.abort();
    app.cancel_inflight().await;
    drop(guard);
    outcome
}

fn flush(guard: &mut TerminalGuard, cells: &[HistoryCell]) -> Result<(), String> {
    if cells.is_empty() {
        return Ok(());
    }
    let terminal = guard.terminal();
    let width = terminal
        .size()
        .map_err(|error| format!("read terminal size: {error}"))?
        .width;
    for cell in cells {
        let lines = cell.to_lines(width);
        let height = lines.len() as u16;
        let paragraph = ratatui::widgets::Paragraph::new(ratatui::text::Text::from(lines));
        terminal
            .insert_before(height, move |buf| {
                use ratatui::widgets::Widget;
                paragraph.render(buf.area, buf);
            })
            .map_err(|error| format!("write transcript to scrollback: {error}"))?;
    }
    Ok(())
}

fn paint(guard: &mut TerminalGuard, app: &App) -> Result<(), String> {
    guard
        .terminal()
        .draw(|frame| render(frame, app))
        .map_err(|error| format!("draw frame: {error}"))?;
    Ok(())
}

fn constraints() -> [Constraint; VIEWPORT_ROWS as usize] {
    [
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ]
}

fn render(frame: &mut Frame<'_>, app: &App) {
    let [status_area, tail_area, composer_area, hint_area] =
        Layout::vertical(constraints()).areas(frame.area());
    frame.render_widget(app.status_line(), status_area);
    frame.render_widget(app.tail_line(tail_area.width), tail_area);
    frame.render_widget(app.composer_line(), composer_area);
    frame.render_widget(app.hint_line(), hint_area);
    if app.overlay_active() {
        return;
    }
    let last_column = composer_area.x + composer_area.width.saturating_sub(1);
    let cursor = (composer_area.x + 2 + display_width(&app.draft) as u16).min(last_column);
    frame.set_cursor_position((cursor, composer_area.y));
}

/// How far a keyboard answer reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalAnswer {
    pub approved: bool,
    /// `None` answers only this call; otherwise the scope string the gate offered.
    pub scope: Option<&'static str>,
}

impl ApprovalAnswer {
    const ONCE: Self = Self {
        approved: true,
        scope: None,
    };
    const DENY: Self = Self {
        approved: false,
        scope: None,
    };
}

/// A keyboard approval decision, honoring only the scopes this request offered.
///
/// The offered list comes from the request DTO, which the gate builds with the
/// same `ApprovalScope::offered` rule it validates answers against — so a key
/// here can never promise something the server would reject.
pub fn approval_decision(key: KeyCode, offered: &[String]) -> Option<ApprovalAnswer> {
    let offers = |scope: &str| offered.iter().any(|candidate| candidate == scope);
    match key {
        KeyCode::Char('y') => Some(ApprovalAnswer::ONCE),
        KeyCode::Char('a') if offers("tool_session") => Some(ApprovalAnswer {
            approved: true,
            scope: Some("tool_session"),
        }),
        KeyCode::Char('p') if offers("command_prefix") => Some(ApprovalAnswer {
            approved: true,
            scope: Some("command_prefix"),
        }),
        KeyCode::Char('n') | KeyCode::Char('d') | KeyCode::Esc => Some(ApprovalAnswer::DENY),
        _ => None,
    }
}

/// Pending approval rendered as an inline prompt. The transcript above stays
/// readable: the run is parked, not replaced.
#[derive(Debug, Clone)]
pub struct ApprovalPrompt {
    pub request: ApprovalRequestDto,
}

impl ApprovalPrompt {
    fn current_line(&self) -> Line<'static> {
        let risk = if self.request.risk.is_empty() {
            String::new()
        } else {
            format!(" [{}]", self.request.risk)
        };
        Line::from(Span::styled(
            format!(
                "allow {}{risk}: {}",
                self.request.tool,
                truncate(&self.request.reason, 60)
            ),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ))
    }
}

/// What the host must do after a question key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionAction {
    Ignore,
    Repaint,
    Decline,
    Answered,
}

/// Question overlay state: which question is showing and which options are
/// toggled for it.
#[derive(Debug, Clone)]
pub struct QuestionPrompt {
    pub request: QuestionRequestDto,
    pub index: usize,
    pub picked: Vec<usize>,
    pub answers: Vec<(String, String)>,
}

impl QuestionPrompt {
    fn new(request: QuestionRequestDto) -> Self {
        Self {
            request,
            index: 0,
            picked: Vec::new(),
            answers: Vec::new(),
        }
    }

    fn current(&self) -> Option<&Question> {
        self.request.questions.get(self.index)
    }

    fn commit_current(&mut self, value: String) {
        if let Some(question) = self.current() {
            self.answers.push((question.question.clone(), value));
            self.index += 1;
            self.picked.clear();
        }
    }

    fn finished(&self) -> bool {
        self.index >= self.request.questions.len()
    }

    fn on_key(&mut self, key: KeyCode) -> QuestionAction {
        let Some(question) = self.current().cloned() else {
            return QuestionAction::Ignore;
        };
        match key {
            KeyCode::Esc | KeyCode::Char('s') => QuestionAction::Decline,
            KeyCode::Char(digit @ ('1'..='9')) => {
                let Some(number) = digit
                    .to_digit(10)
                    .and_then(|digit| usize::try_from(digit - 1).ok())
                else {
                    return QuestionAction::Ignore;
                };
                if number >= question.options.len() {
                    return QuestionAction::Ignore;
                }
                if !question.multi_select {
                    self.commit_current(question.options[number].label.clone());
                    return if self.finished() {
                        QuestionAction::Answered
                    } else {
                        QuestionAction::Repaint
                    };
                }
                if let Some(at) = self.picked.iter().position(|picked| *picked == number) {
                    self.picked.remove(at);
                } else {
                    self.picked.push(number);
                }
                self.picked.sort_unstable();
                QuestionAction::Repaint
            }
            KeyCode::Enter => {
                if !question.multi_select || self.picked.is_empty() {
                    return QuestionAction::Ignore;
                }
                let labels = self
                    .picked
                    .iter()
                    .filter_map(|number| question.options.get(*number))
                    .map(|option| option.label.clone())
                    .collect();
                self.commit_current(join_selection(labels));
                if self.finished() {
                    QuestionAction::Answered
                } else {
                    QuestionAction::Repaint
                }
            }
            _ => QuestionAction::Ignore,
        }
    }

    fn take_answers(&mut self) -> Option<std::collections::BTreeMap<String, String>> {
        self.finished().then(|| self.answers.drain(..).collect())
    }

    fn current_line(&self) -> Line<'static> {
        let Some(question) = self.current() else {
            return Line::from("");
        };
        let options = question
            .options
            .iter()
            .enumerate()
            .map(|(number, option)| {
                let mark = if self.picked.contains(&number) {
                    "*"
                } else {
                    ""
                };
                format!("{mark}{}) {}", number + 1, option.label)
            })
            .collect::<Vec<_>>()
            .join("   ");
        let mut spans = vec![
            Span::styled(
                format!(
                    "{} ({}/{})  ",
                    question.question,
                    self.index + 1,
                    self.request.questions.len()
                ),
                Style::default().fg(Color::White),
            ),
            Span::styled(options, Style::default().fg(Color::Cyan)),
        ];
        if let Some(description) = self.current_description() {
            spans.push(Span::styled(
                format!("  — {description}"),
                Style::default().fg(Color::DarkGray),
            ));
        }
        Line::from(spans)
    }

    fn current_description(&self) -> Option<String> {
        let picked = self.current()?.options.get(*self.picked.first()?)?;
        (!picked.description.is_empty()).then(|| picked.description.clone())
    }
}

pub struct App {
    workspace: PathBuf,
    /// Which database this host actually opened: the desktop and the terminal must
    /// be provably the same file, so the path is on screen, not in a log.
    data_root: String,
    model: String,
    sandbox: String,
    overrides: HarnessRunOverrides,
    chat: ChatService,
    ui_sender: UnboundedSender<UiEvent>,
    state: ChatState,
    draft: String,
    inflight: Option<tokio::task::JoinHandle<()>>,
    flush: Vec<HistoryCell>,
    approval: Option<ApprovalPrompt>,
    question: Option<QuestionPrompt>,
    spinner: usize,
    pub dirty: bool,
    should_quit: bool,
}

impl App {
    fn new(
        workspace: PathBuf,
        options: &RunOptions,
        chat: ChatService,
        ui_sender: UnboundedSender<UiEvent>,
    ) -> Self {
        let model = options
            .model
            .clone()
            .unwrap_or_else(|| "from settings".to_string());
        let sandbox = options
            .sandbox_backend
            .clone()
            .unwrap_or_else(|| "direct".to_string());
        let overrides = HarnessRunOverrides {
            provider: options.provider.clone(),
            model: options.model.clone(),
            reasoning_effort: options.reasoning_effort.clone(),
            sandbox_backend: options.sandbox_backend.clone(),
            permission_profile: options.permission_profile.clone(),
        };
        let thread_id = options.continue_thread.clone();
        Self {
            workspace,
            data_root: chat.database().data_root().display().to_string(),
            model,
            sandbox,
            overrides,
            chat,
            ui_sender,
            state: ChatState::new(thread_id),
            draft: String::new(),
            inflight: None,
            flush: Vec::new(),
            approval: None,
            question: None,
            spinner: 0,
            dirty: true,
            should_quit: false,
        }
    }

    fn on_tui_event(&mut self, event: TuiEvent) -> bool {
        match event {
            TuiEvent::Key(key) => self.on_key(key),
            TuiEvent::Paste(text) => {
                if !self.overlay_active() {
                    self.draft.push_str(&text);
                    self.dirty = true;
                }
                false
            }
            TuiEvent::Resize => {
                self.dirty = true;
                false
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent) -> bool {
        if let Some(prompt) = &mut self.question {
            let action = prompt.on_key(key.code);
            match action {
                QuestionAction::Ignore => {}
                QuestionAction::Repaint => self.dirty = true,
                QuestionAction::Decline => self.resolve_question(None),
                QuestionAction::Answered => {
                    let answers = prompt.take_answers();
                    self.resolve_question(answers);
                }
            }
            return false;
        }
        let offered = self
            .approval
            .as_ref()
            .map(|prompt| prompt.request.scopes.clone());
        if let Some(offered) = offered {
            if let Some(answer) = approval_decision(key.code, &offered) {
                self.resolve_approval(answer);
            }
            return false;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('c') if ctrl => {
                if self.state.busy {
                    self.request_interrupt();
                } else if self.draft.is_empty() {
                    self.should_quit = true;
                } else {
                    self.draft.clear();
                    self.set_notice("draft cleared");
                    self.dirty = true;
                }
            }
            KeyCode::Char('d') if ctrl => self.should_quit = true,
            KeyCode::Esc if self.state.busy => self.request_interrupt(),
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace => {
                self.draft.pop();
                self.dirty = true;
            }
            KeyCode::Char(ch) if !ctrl && !alt => {
                self.draft.push(ch);
                self.dirty = true;
            }
            _ => {}
        }
        self.should_quit
    }

    fn on_tick(&mut self) -> bool {
        if !self.state.busy {
            return false;
        }
        self.spinner = (self.spinner + 1) % SPINNER.len();
        self.dirty = true;
        false
    }

    fn on_ui_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Runtime(runtime_event) => {
                let terminal = matches!(
                    runtime_event,
                    deepagent_runtime::RuntimeEvent::RunCompleted { .. }
                        | deepagent_runtime::RuntimeEvent::RunFailed { .. }
                        | deepagent_runtime::RuntimeEvent::RunCancelled { .. }
                );
                let continue_after = self.state.on_runtime(runtime_event);
                if terminal {
                    self.approval = None;
                    self.question = None;
                }
                if continue_after == Continue::DrainQueue {
                    self.start_next_queued();
                }
            }
            UiEvent::Approval(request) => {
                self.approval = Some(ApprovalPrompt { request });
            }
            UiEvent::Question(request) => {
                self.question = Some(QuestionPrompt::new(request));
            }
            UiEvent::Finished(result) => {
                self.state.on_finished(result);
                self.approval = None;
                self.question = None;
                self.start_next_queued();
            }
        }
        self.dirty = true;
    }

    fn submit(&mut self) {
        let prompt = self.draft.trim().to_string();
        if prompt.is_empty() {
            return;
        }
        self.draft.clear();
        self.start_prompt(prompt);
    }

    /// Record the prompt, then either start it or queue it behind the run in
    /// flight. There is no in-turn injection channel, so a queued prompt becomes
    /// the next turn of the same thread.
    fn start_prompt(&mut self, prompt: String) {
        self.state.record_prompt(prompt.clone());
        match self.state.submit(prompt) {
            Some(to_run) => self.spawn(to_run),
            None => self.set_notice("queued for the next turn"),
        }
        self.dirty = true;
    }

    fn start_next_queued(&mut self) {
        while let Some(prompt) = self.take_next_to_run() {
            self.spawn(prompt);
        }
    }

    /// Claim the next queued prompt for this host, or `None` while a run is still
    /// in flight. Split out of `start_next_queued` so the queueing rule can be
    /// tested without launching a run.
    fn take_next_to_run(&mut self) -> Option<String> {
        if self.state.busy {
            return None;
        }
        let prompt = self.state.take_next_queued()?;
        self.state.submit(prompt.clone());
        Some(prompt)
    }

    fn spawn(&mut self, prompt: String) {
        self.inflight = Some(host::start_turn(
            self.chat.clone(),
            prompt,
            self.state.thread_id.clone(),
            self.overrides.clone(),
            self.ui_sender.clone(),
        ));
    }

    fn request_interrupt(&mut self) {
        let Some(target) = self.interrupt_target() else {
            self.set_notice("nothing to interrupt");
            self.dirty = true;
            return;
        };
        match self.chat.request_cancel(&target) {
            Ok(request) if request.accepted => self.set_notice("interrupt requested"),
            Ok(_) => self.set_notice(format!("no in-flight run to interrupt: {target}")),
            Err(error) => self.set_notice(format!("interrupt failed: {error}")),
        }
        self.dirty = true;
    }

    fn interrupt_target(&self) -> Option<String> {
        self.state
            .run_id
            .clone()
            .or_else(|| self.state.thread_id.clone())
    }

    fn resolve_approval(&mut self, answer: ApprovalAnswer) {
        let Some(prompt) = self.approval.take() else {
            return;
        };
        let outcome = self.chat.resolve_approval_scoped(
            &prompt.request.call_id,
            answer.approved,
            answer.scope,
            "cli_tui_user",
        );
        match outcome {
            Ok(true) => self.set_notice(match (answer.approved, answer.scope) {
                (true, Some("tool_session")) => {
                    "approved, and this tool is allowed for the rest of this session".to_string()
                }
                (true, Some(_)) | (true, None) => "approved".to_string(),
                (false, _) => "denied".to_string(),
            }),
            Ok(false) => self.set_notice("approval was no longer pending"),
            Err(error) => self.set_notice(format!("could not record the approval: {error}")),
        }
        self.dirty = true;
    }

    fn resolve_question(&mut self, answers: Option<std::collections::BTreeMap<String, String>>) {
        let Some(prompt) = self.question.take() else {
            return;
        };
        if !self
            .chat
            .resolve_question(&prompt.request.question_id, answers.clone())
        {
            self.set_notice("question was no longer pending");
        } else if answers.is_none() {
            self.set_notice("declined to answer");
        }
        self.dirty = true;
    }

    fn set_notice(&mut self, text: impl Into<String>) {
        self.state.notice = Some(text.into());
    }

    fn overlay_active(&self) -> bool {
        self.question.is_some() || self.approval.is_some()
    }

    /// Ask the run to stop, then give it a moment to persist its own cancelled
    /// terminal state. A run that ignores cancellation is left to the next host's
    /// startup recovery, which is exactly what the primary-host lock protects.
    async fn cancel_inflight(&mut self) {
        if let Some(target) = self.interrupt_target() {
            let _ = self.chat.request_cancel(&target);
        }
        let Some(handle) = self.inflight.take() else {
            return;
        };
        if let Err(error) = tokio::time::timeout(Duration::from_millis(500), handle).await {
            tracing::warn!("run task was still alive at shutdown: {error}");
        }
    }

    /// Cells that will not change again are handed over once, in transcript order.
    fn collect_flushable(&mut self) {
        self.flush.extend(self.state.drain_flushable());
    }

    fn take_flushable(&mut self) -> Vec<HistoryCell> {
        std::mem::take(&mut self.flush)
    }

    fn status_line(&self) -> Line<'static> {
        let muted = Style::default().fg(Color::DarkGray);
        let value = Style::default().fg(Color::White);
        let mut spans = vec![
            Span::styled("DeepAgent", Style::default().fg(Color::Cyan)),
            Span::styled("  ", muted),
            Span::styled(truncate(&self.workspace.display().to_string(), 16), muted),
            Span::styled("  db: ", muted),
            Span::styled(tail_path(&self.data_root, 26), muted),
            Span::styled("  model: ", muted),
            Span::styled(truncate(&self.model, 20), value),
            Span::styled("  sandbox: ", muted),
            Span::styled(self.sandbox.clone(), value),
        ];
        if self.state.busy {
            spans.push(Span::styled(
                format!("  {} working", SPINNER[self.spinner]),
                Style::default().fg(Color::Yellow),
            ));
        }
        if !self.state.queued.is_empty() {
            spans.push(Span::styled(
                format!("  queued: {}", self.state.queued.len()),
                muted,
            ));
        }
        if self.state.tokens.total() > 0 {
            let cache = self
                .state
                .tokens
                .cache_hit_percent()
                .map(|percent| format!(" (cache {percent}%)"))
                .unwrap_or_default();
            spans.push(Span::styled(
                format!("  tok: {}{cache}", self.state.tokens.total()),
                muted,
            ));
        }
        Line::from(spans)
    }

    /// The live tail of the transcript: an in-flight tool row, or whatever text is
    /// still streaming.
    fn tail_line(&self, width: u16) -> Line<'static> {
        if let Some(cell) = self.state.running_tool() {
            if let Some(line) = cell.to_lines(width).into_iter().next() {
                return line;
            }
        }
        let streaming = !self.state.streaming.trim().is_empty();
        let text = if streaming {
            self.state.streaming.as_str()
        } else if !self.state.reasoning.trim().is_empty() {
            self.state.reasoning.as_str()
        } else {
            self.state.notice.as_deref().unwrap_or("")
        };
        if text.is_empty() {
            return Line::from("");
        }
        let style = if streaming {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let flattened = text.replace('\n', " ");
        Line::from(Span::styled(
            truncate(&sanitize(&flattened), width as usize),
            style,
        ))
    }

    fn composer_line(&self) -> Line<'static> {
        if let Some(prompt) = &self.question {
            return prompt.current_line();
        }
        if let Some(prompt) = &self.approval {
            return prompt.current_line();
        }
        Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Yellow)),
            Span::styled(
                truncate(&sanitize(&self.draft.replace('\n', " ")), 200),
                Style::default().fg(Color::White),
            ),
        ])
    }

    fn hint_line(&self) -> Line<'static> {
        let text = if self.question.is_some() {
            "1-9 choose | Enter confirm multi | s decline".to_string()
        } else if let Some(prompt) = self.approval.as_ref() {
            // Only keys that will actually be honored are advertised.
            let mut keys = String::from("y allow once");
            if prompt
                .request
                .scopes
                .iter()
                .any(|scope| scope == "tool_session")
            {
                keys.push_str(" | a allow for this session");
            }
            if prompt
                .request
                .scopes
                .iter()
                .any(|scope| scope == "command_prefix")
            {
                keys.push_str(" | p trust this command");
            }
            keys.push_str(" | n/d/Esc deny");
            keys
        } else if self.state.busy {
            "Enter queue a follow-up | Esc interrupt the run".to_string()
        } else {
            "Enter send | Ctrl-C / Ctrl-D quit".to_string()
        };
        Line::from(Span::styled(text, Style::default().fg(Color::DarkGray)))
    }
}

/// Strip control characters that would corrupt the terminal grid.
fn sanitize(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_control()).collect()
}

/// Show the *end* of a path. For a data root the distinguishing part is the
/// tail (`…\com.deepagent.studio`), while the head is the same user profile
/// directory every time.
fn tail_path(path: &str, width: usize) -> String {
    if display_width(path) <= width || width < 2 {
        return truncate(path, width);
    }
    let chars: Vec<char> = path.chars().collect();
    let keep = (width - 1).min(chars.len());
    let mut out = String::from('…');
    out.extend(chars[chars.len() - keep..].iter());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::text::Text;
    use ratatui::widgets::{Paragraph, Widget};
    use ratatui::{Terminal, TerminalOptions, Viewport};

    fn approval_request() -> ApprovalRequestDto {
        ApprovalRequestDto {
            call_id: "call-1".to_string(),
            tool: "run_command".to_string(),
            risk: "high".to_string(),
            arguments: "{\"command\":\"rm -rf target\"}".to_string(),
            reason: "destructive command".to_string(),
            scopes: vec!["single_call".to_string(), "tool_session".to_string()],
        }
    }

    fn option(label: &str, description: &str) -> deepagent_builtins::QuestionOption {
        deepagent_builtins::QuestionOption {
            label: label.to_string(),
            description: description.to_string(),
        }
    }

    fn question_request(count: usize, multi: bool) -> QuestionRequestDto {
        QuestionRequestDto {
            question_id: "q-1".to_string(),
            questions: (0..count)
                .map(|index| Question {
                    question: format!("Pick {index}?"),
                    header: String::new(),
                    options: vec![option("Alpha", "first"), option("Beta", "")],
                    multi_select: multi,
                })
                .collect(),
        }
    }

    fn joined(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn approval_keys_honor_only_the_scopes_the_request_offered() {
        let once = || ApprovalAnswer {
            approved: true,
            scope: None,
        };
        let deny = || ApprovalAnswer {
            approved: false,
            scope: None,
        };
        let offered = |scopes: &[&str]| {
            scopes
                .iter()
                .map(|scope| scope.to_string())
                .collect::<Vec<String>>()
        };

        assert_eq!(
            approval_decision(KeyCode::Char('y'), &offered(&["single_call"])),
            Some(once())
        );
        for key in [KeyCode::Char('n'), KeyCode::Char('d'), KeyCode::Esc] {
            assert_eq!(approval_decision(key, &offered(&[])), Some(deny()));
        }
        // `a` answers only when the request advertised the session scope.
        assert_eq!(
            approval_decision(
                KeyCode::Char('a'),
                &offered(&["single_call", "tool_session"])
            ),
            Some(ApprovalAnswer {
                approved: true,
                scope: Some("tool_session")
            })
        );
        assert_eq!(
            approval_decision(KeyCode::Char('a'), &offered(&["single_call"])),
            None
        );
        assert_eq!(
            approval_decision(KeyCode::Char('p'), &offered(&["command_prefix"])),
            Some(ApprovalAnswer {
                approved: true,
                scope: Some("command_prefix")
            })
        );
        assert_eq!(
            approval_decision(KeyCode::Char('z'), &offered(&["tool_session"])),
            None
        );
    }

    #[test]
    fn the_approval_hint_advertises_only_the_live_keys() {
        let mut app = test_app();
        let prompt = |scopes: Vec<String>| ApprovalPrompt {
            request: ApprovalRequestDto {
                call_id: "c1".into(),
                tool: "bash".into(),
                risk: "high".into(),
                arguments: "{}".into(),
                reason: "destructive command".into(),
                scopes,
            },
        };
        app.approval = Some(prompt(vec!["single_call".to_string()]));
        let plain = joined(&app.hint_line());
        assert!(plain.contains("y allow once"), "{plain}");
        assert!(!plain.contains(" a allow"), "{plain}");
        app.approval = Some(prompt(vec![
            "single_call".to_string(),
            "tool_session".to_string(),
        ]));
        assert!(joined(&app.hint_line()).contains("a allow for this session"));
    }

    #[test]
    fn single_select_answers_every_question_in_order() {
        let mut prompt = QuestionPrompt::new(question_request(2, false));
        assert_eq!(prompt.on_key(KeyCode::Char('1')), QuestionAction::Repaint);
        assert_eq!(prompt.on_key(KeyCode::Char('2')), QuestionAction::Answered);
        assert_eq!(
            prompt.take_answers(),
            Some(std::collections::BTreeMap::from([
                ("Pick 0?".to_string(), "Alpha".to_string()),
                ("Pick 1?".to_string(), "Beta".to_string()),
            ]))
        );
    }

    #[test]
    fn multi_select_toggles_then_confirms_on_enter() {
        let mut prompt = QuestionPrompt::new(question_request(1, true));
        assert_eq!(prompt.on_key(KeyCode::Char('1')), QuestionAction::Repaint);
        assert_eq!(prompt.on_key(KeyCode::Char('2')), QuestionAction::Repaint);
        assert_eq!(prompt.on_key(KeyCode::Char('2')), QuestionAction::Repaint);
        assert_eq!(prompt.picked, vec![0]);
        assert_eq!(prompt.on_key(KeyCode::Enter), QuestionAction::Answered);
        assert_eq!(
            prompt.take_answers(),
            Some(std::collections::BTreeMap::from([(
                "Pick 0?".to_string(),
                "Alpha".to_string()
            )]))
        );
    }

    #[test]
    fn multi_select_rejects_enter_with_nothing_picked() {
        let mut prompt = QuestionPrompt::new(question_request(1, true));
        assert_eq!(prompt.on_key(KeyCode::Enter), QuestionAction::Ignore);
    }

    #[test]
    fn out_of_range_and_unrelated_keys_are_ignored() {
        let mut prompt = QuestionPrompt::new(question_request(1, false));
        assert_eq!(prompt.on_key(KeyCode::Char('7')), QuestionAction::Ignore);
        assert_eq!(prompt.on_key(KeyCode::Tab), QuestionAction::Ignore);
        assert!(prompt.answers.is_empty());
    }

    #[test]
    fn escape_or_s_declines_the_whole_set() {
        let mut prompt = QuestionPrompt::new(question_request(2, false));
        assert_eq!(prompt.on_key(KeyCode::Esc), QuestionAction::Decline);
        let mut prompt = QuestionPrompt::new(question_request(2, false));
        assert_eq!(prompt.on_key(KeyCode::Char('s')), QuestionAction::Decline);
    }

    #[test]
    fn picked_option_description_shows_under_the_question() {
        let mut prompt = QuestionPrompt::new(question_request(1, true));
        assert_eq!(prompt.current_description(), None);
        prompt.on_key(KeyCode::Char('1'));
        assert_eq!(prompt.current_description().as_deref(), Some("first"));
    }

    #[test]
    fn question_line_numbers_the_current_step_and_marks_picks() {
        let mut prompt = QuestionPrompt::new(question_request(2, true));
        prompt.on_key(KeyCode::Char('2'));
        let line = joined(&prompt.current_line());
        assert!(line.contains("Pick 0? (1/2)"), "{line}");
        assert!(line.contains("*2) Beta"), "{line}");
    }

    #[test]
    fn approval_prompt_names_the_tool_risk_and_reason() {
        let line = joined(
            &ApprovalPrompt {
                request: approval_request(),
            }
            .current_line(),
        );
        assert!(line.contains("allow run_command [high]"));
        assert!(line.contains("destructive command"));
    }

    #[test]
    fn control_characters_never_reach_the_grid() {
        assert_eq!(sanitize("a\u{7}b\n"), "ab");
    }

    fn test_app() -> App {
        let dir = tempfile::tempdir().expect("temp dir");
        let database = std::sync::Arc::new(
            deepagent_persistence::Database::open_in_memory().expect("test database"),
        );
        let transport: std::sync::Arc<dyn deepagent_models::HttpTransport> =
            std::sync::Arc::new(deepagent_models::ReqwestTransport::new());
        let settings = std::sync::Arc::new(deepagent_app_core::SettingsService::new(
            std::sync::Arc::clone(&database),
            std::sync::Arc::clone(&transport),
            std::sync::Arc::new(deepagent_app_core::EnvSecretStore::new()),
        ));
        let chat = ChatService::new(database, settings, transport, dir.path().to_path_buf());
        let (ui_sender, _ui_receiver) = tokio::sync::mpsc::unbounded_channel::<UiEvent>();
        let options = RunOptions {
            prompt: String::new(),
            continue_thread: None,
            json: false,
            provider: None,
            model: Some("deepseek-chat".to_string()),
            sandbox_backend: Some("direct".to_string()),
            permission_profile: None,
            reasoning_effort: None,
        };
        let mut app = App::new(dir.path().to_path_buf(), &options, chat, ui_sender);
        // `Database::open_in_memory` roots itself in a fresh temp dir, so leaving
        // the derived root in place would bake a random path into every snapshot.
        app.data_root = "/home/dev/.deepagent".to_string();
        // The workspace directory must outlive the service built on it.
        std::mem::forget(dir);
        app
    }

    #[test]
    fn a_follow_up_prompt_queues_instead_of_starting_a_second_run() {
        let mut app = test_app();
        app.state.busy = true;
        app.start_prompt("second prompt".to_string());
        assert_eq!(app.state.queued, vec!["second prompt".to_string()]);
        assert!(
            app.state.notice.is_some(),
            "the queue has to be visible to the user"
        );
        assert_eq!(
            app.state.cells.len(),
            1,
            "a queued prompt records exactly one transcript cell"
        );
    }

    #[test]
    fn queued_prompts_never_start_while_a_run_is_in_flight() {
        let mut app = test_app();
        app.state.busy = true;
        app.start_prompt("queued".to_string());
        assert_eq!(app.state.queued.len(), 1);
        assert!(
            app.take_next_to_run().is_none(),
            "a busy host must not start a second run"
        );
        app.state.busy = false;
        assert_eq!(app.take_next_to_run().as_deref(), Some("queued"));
        assert!(app.state.busy, "the follow-up owns the run slot now");
        assert!(app.state.queued.is_empty());
    }

    #[test]
    fn interrupt_without_any_run_says_so() {
        let mut app = test_app();
        app.request_interrupt();
        assert_eq!(app.state.notice.as_deref(), Some("nothing to interrupt"));
    }

    #[test]
    fn streamed_text_shows_in_the_tail_until_it_is_committed() {
        let mut app = test_app();
        app.state.streaming = "thinking out loud".to_string();
        assert_eq!(joined(&app.tail_line(60)), "thinking out loud");
        app.state
            .on_runtime(deepagent_runtime::RuntimeEvent::RunCompleted {
                message: String::new(),
                edited_files: vec![],
            });
        assert_eq!(joined(&app.tail_line(60)), "");
        assert_eq!(app.state.cells.len(), 1, "the streamed text became a row");
    }

    #[test]
    fn an_in_flight_tool_row_shows_in_the_tail_until_it_settles() {
        let mut app = test_app();
        app.state
            .on_runtime(deepagent_runtime::RuntimeEvent::ToolStarted {
                name: "run_command".to_string(),
                call_id: "c1".to_string(),
                arguments: serde_json::Value::Null,
                tool_kind: None,
                file_path: None,
                summary: Some("cargo build".to_string()),
                meta: None,
            });
        let tail = joined(&app.tail_line(60));
        assert!(tail.contains("run_command: cargo build"), "{tail}");
        assert!(tail.contains("running"), "{tail}");
    }

    #[test]
    fn status_line_shows_which_database_this_host_opened() {
        let app = test_app();
        let line = joined(&app.status_line());
        assert!(line.contains("db: "), "{line}");
        assert!(
            !app.data_root.is_empty(),
            "the resolved data root must always be on screen"
        );
    }

    #[test]
    fn tail_path_keeps_the_distinguishing_end() {
        let long = r"C:\Users\me\AppData\Roaming\com.deepagent.studio";
        let shown = tail_path(long, 26);
        assert!(shown.starts_with('…'), "{shown}");
        assert!(shown.ends_with("com.deepagent.studio"), "{shown}");
        assert!(display_width(&shown) <= 26, "{shown}");
        assert_eq!(tail_path("short", 26), "short");
        assert_eq!(tail_path("short", 0), "");
    }

    #[test]
    fn status_line_reports_tokens_and_cache_hits() {
        let mut app = test_app();
        app.state
            .on_runtime(deepagent_runtime::RuntimeEvent::Usage {
                prompt_tokens: 1000,
                completion_tokens: 200,
                reasoning_tokens: 20,
                total_tokens: 1200,
                prompt_cache_hit_tokens: 750,
                prompt_cache_miss_tokens: 250,
                cost_yuan: None,
                raw_responses_usage: None,
            });
        let line = joined(&app.status_line());
        assert!(line.contains("tok: 1200"), "{line}");
        assert!(line.contains("cache 75%"), "{line}");
    }

    #[test]
    fn status_line_carries_model_and_working_indicator() {
        let mut app = test_app();
        let line = joined(&app.status_line());
        assert!(line.contains("model: deepseek-chat"), "{line}");
        assert!(!line.contains("working"));
        app.state.busy = true;
        assert!(joined(&app.status_line()).contains("working"));
    }

    #[test]
    fn overlay_replaces_the_composer_and_the_hint() {
        let mut app = test_app();
        app.approval = Some(ApprovalPrompt {
            request: approval_request(),
        });
        assert!(joined(&app.composer_line()).contains("allow run_command"));
        assert!(joined(&app.hint_line()).contains("y allow once"));
        assert!(app.overlay_active());
    }

    #[test]
    fn settled_cells_are_collected_once_in_order() {
        let mut app = test_app();
        app.state.record_prompt("first".to_string());
        app.collect_flushable();
        assert_eq!(app.take_flushable().len(), 1);
        assert!(
            app.take_flushable().is_empty(),
            "a flushed cell must not be written twice"
        );
    }

    #[test]
    fn viewport_snapshot_with_a_pending_approval() {
        let mut terminal = Terminal::with_options(
            TestBackend::new(72, 10),
            TerminalOptions {
                viewport: Viewport::Inline(VIEWPORT_ROWS),
            },
        )
        .expect("test backend terminal");
        let mut app = test_app();
        app.state.record_prompt("改一下渲染".to_string());
        app.state
            .on_runtime(deepagent_runtime::RuntimeEvent::ToolStarted {
                name: "edit_file".to_string(),
                call_id: "c1".to_string(),
                arguments: serde_json::Value::Null,
                tool_kind: None,
                file_path: None,
                summary: Some("src/tui/app.rs".to_string()),
                meta: None,
            });
        app.state.streaming = "我来改这个文件".to_string();
        app.approval = Some(ApprovalPrompt {
            request: approval_request(),
        });
        app.collect_flushable();
        let cells = app.take_flushable();
        for cell in &cells {
            let lines = cell.to_lines(72);
            let height = lines.len() as u16;
            let paragraph = Paragraph::new(Text::from(lines));
            terminal
                .insert_before(height, move |buf| {
                    paragraph.render(buf.area, buf);
                })
                .expect("flush cell");
        }
        terminal
            .draw(|frame: &mut Frame<'_>| render(frame, &app))
            .expect("draw overlay frame");
        insta::assert_snapshot!(terminal.backend().to_string());
    }
}
