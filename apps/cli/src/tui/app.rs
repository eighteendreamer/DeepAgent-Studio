use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc;

use crate::args::RunOptions;
use crate::tui::event::{self, TuiEvent};
use crate::tui::history::{display_width, truncate, HistoryCell};
use crate::tui::terminal::TerminalGuard;

/// Rows owned by the live viewport: status, streaming tail, composer, hint.
const VIEWPORT_ROWS: u16 = 4;

pub async fn run(options: RunOptions) -> Result<(), String> {
    let workspace =
        std::env::current_dir().map_err(|error| format!("read current directory: {error}"))?;
    let mut app = App::new(workspace, &options);
    let mut guard = TerminalGuard::inline(VIEWPORT_ROWS)?;

    let (sender, mut receiver) = mpsc::channel::<TuiEvent>(64);
    let pump = event::spawn_pump(sender);

    paint(guard.terminal(), &mut app)?;
    let mut outcome = Ok(());
    while let Some(event) = receiver.recv().await {
        let quit = match event {
            TuiEvent::Key(key) => app.on_key(key),
            TuiEvent::Paste(text) => {
                app.on_paste(&text);
                false
            }
            // Terminal::draw autoresizes, so a resize only needs another paint pass.
            TuiEvent::Resize => false,
        };
        if let Err(error) = paint(guard.terminal(), &mut app) {
            outcome = Err(error);
            break;
        }
        if quit {
            break;
        }
    }
    pump.abort();
    drop(guard);
    outcome
}

fn constraints() -> [Constraint; VIEWPORT_ROWS as usize] {
    [
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ]
}

/// Paints one frame: committed transcript first, then the small live viewport.
fn paint<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<(), String> {
    flush(terminal, app)?;
    terminal
        .draw(|frame| render(frame, app))
        .map_err(|error| format!("draw frame: {error}"))?;
    Ok(())
}

/// Writes cells that finished since the last frame into the terminal's own
/// scrollback, keeping the viewport small and the history mouse-scrollable.
fn flush<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<(), String> {
    if app.pending.is_empty() {
        return Ok(());
    }
    let width = terminal
        .size()
        .map_err(|error| format!("read terminal size: {error}"))?
        .width;
    for cell in std::mem::take(&mut app.pending) {
        let lines = cell.to_lines(width);
        let height = lines.len() as u16;
        let paragraph = Paragraph::new(Text::from(lines));
        terminal
            .insert_before(height, move |buf| {
                paragraph.render(buf.area, buf);
            })
            .map_err(|error| format!("write transcript to scrollback: {error}"))?;
    }
    Ok(())
}

fn render(frame: &mut Frame<'_>, app: &App) {
    let [status_area, tail_area, composer_area, hint_area] =
        Layout::vertical(constraints()).areas(frame.area());
    frame.render_widget(app.status_line(), status_area);
    frame.render_widget(app.tail_line(tail_area.width as usize), tail_area);
    frame.render_widget(app.composer_line(), composer_area);
    frame.render_widget(app.hint_line(), hint_area);
    let last_column = composer_area.x + composer_area.width.saturating_sub(1);
    let cursor = (composer_area.x + 2 + display_width(&app.draft) as u16).min(last_column);
    frame.set_cursor_position((cursor, composer_area.y));
}

#[derive(Debug)]
pub struct App {
    workspace: PathBuf,
    model: String,
    sandbox: String,
    draft: String,
    pending: Vec<HistoryCell>,
    notice: Option<String>,
}

impl App {
    fn new(workspace: PathBuf, options: &RunOptions) -> Self {
        let model = options
            .model
            .clone()
            .unwrap_or_else(|| "from settings".to_string());
        let sandbox = options
            .sandbox_backend
            .clone()
            .unwrap_or_else(|| "direct".to_string());
        Self {
            workspace,
            model,
            sandbox,
            draft: String::new(),
            pending: Vec::new(),
            notice: None,
        }
    }

    fn on_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('c') if ctrl => {
                if self.draft.is_empty() {
                    return true;
                }
                self.draft.clear();
                self.set_notice("draft cleared; Ctrl-C again quits");
            }
            KeyCode::Char('d') if ctrl => return true,
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace => {
                self.draft.pop();
            }
            KeyCode::Char(ch) if !ctrl && !alt => self.draft.push(ch),
            _ => {}
        }
        false
    }

    fn on_paste(&mut self, text: &str) {
        self.draft.push_str(text);
        self.set_notice("pasted into the composer; Enter records the prompt");
    }

    fn submit(&mut self) {
        let prompt = self.draft.trim().to_string();
        if prompt.is_empty() {
            return;
        }
        self.draft.clear();
        self.pending.push(HistoryCell::UserPrompt { text: prompt });
        self.set_notice("model run wiring lands in the next phase: this prompt was not sent");
    }

    fn set_notice(&mut self, text: &str) {
        self.notice = Some(text.to_string());
    }

    fn status_line(&self) -> Line<'static> {
        let muted = Style::default().fg(Color::DarkGray);
        let value = Style::default().fg(Color::White);
        let gap = Span::styled("  ", muted);
        Line::from(vec![
            Span::styled("DeepAgent", Style::default().fg(Color::Cyan)),
            gap.clone(),
            Span::styled(truncate(&self.workspace.display().to_string(), 28), muted),
            gap.clone(),
            Span::styled("model: ", muted),
            Span::styled(self.model.clone(), value),
            gap,
            Span::styled("sandbox: ", muted),
            Span::styled(self.sandbox.clone(), value),
        ])
    }

    fn tail_line(&self, width: usize) -> Line<'static> {
        match &self.notice {
            Some(notice) => Line::from(Span::styled(
                truncate(notice, width),
                Style::default().fg(Color::DarkGray),
            )),
            None => Line::from(""),
        }
    }

    fn composer_line(&self) -> Line<'static> {
        Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Yellow)),
            Span::styled(self.draft.clone(), Style::default().fg(Color::White)),
        ])
    }

    fn hint_line(&self) -> Line<'static> {
        Line::from(Span::styled(
            "Enter record | Ctrl-C clear then quit | Ctrl-D quit",
            Style::default().fg(Color::DarkGray),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::{TerminalOptions, Viewport};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn app() -> App {
        App::new(
            PathBuf::from("G:/repo"),
            &RunOptions {
                prompt: String::new(),
                continue_thread: None,
                json: false,
                provider: None,
                model: Some("deepseek-chat".to_string()),
                sandbox_backend: Some("sandboxie".to_string()),
                permission_profile: None,
                reasoning_effort: None,
            },
        )
    }

    fn test_terminal(width: u16, height: u16) -> Terminal<TestBackend> {
        Terminal::with_options(
            TestBackend::new(width, height),
            TerminalOptions {
                viewport: Viewport::Inline(VIEWPORT_ROWS),
            },
        )
        .expect("test backend terminal")
    }

    #[test]
    fn ctrl_c_clears_the_draft_before_it_quits() {
        let mut app = app();
        app.draft.push_str("draft");
        assert!(!app.on_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        assert!(app.draft.is_empty());
        assert!(app.on_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn ctrl_d_always_quits() {
        let mut app = app();
        app.draft.push_str("draft");
        assert!(app.on_key(key(KeyCode::Char('d'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn enter_records_a_cell_and_clears_the_composer() {
        let mut app = app();
        app.draft.push_str("  inspect repo  ");
        assert!(!app.on_key(key(KeyCode::Enter, KeyModifiers::NONE)));
        assert!(app.draft.is_empty());
        assert_eq!(
            app.pending,
            vec![HistoryCell::UserPrompt {
                text: "inspect repo".to_string()
            }]
        );
    }

    #[test]
    fn blank_enter_records_nothing() {
        let mut app = app();
        app.draft.push_str("   ");
        app.on_key(key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.pending.is_empty());
    }

    #[test]
    fn printable_keys_edit_the_draft() {
        let mut app = app();
        app.on_key(key(KeyCode::Char('你'), KeyModifiers::NONE));
        app.on_key(key(KeyCode::Char('好'), KeyModifiers::NONE));
        app.on_key(key(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.draft, "你");
    }

    #[test]
    fn alt_and_function_keys_are_ignored() {
        let mut app = app();
        assert!(!app.on_key(key(KeyCode::F(5), KeyModifiers::NONE)));
        assert!(!app.on_key(key(KeyCode::Char('x'), KeyModifiers::ALT)));
        assert!(app.draft.is_empty());
    }

    #[test]
    fn paste_lands_in_the_composer() {
        let mut app = app();
        app.on_paste("line one\nline two");
        assert_eq!(app.draft, "line one\nline two");
        assert!(app.notice.is_some());
    }

    #[test]
    fn status_line_reports_the_configured_model_and_backend() {
        let rendered: String = app()
            .status_line()
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(rendered.contains("deepseek-chat"));
        assert!(rendered.contains("sandboxie"));
    }

    #[test]
    fn viewport_layout_has_one_row_per_viewport_row() {
        assert_eq!(constraints().len(), VIEWPORT_ROWS as usize);
    }

    #[test]
    fn empty_viewport_snapshot() {
        let mut terminal = test_terminal(56, 10);
        let mut app = app();
        paint(&mut terminal, &mut app).expect("paint");
        insta::assert_snapshot!(terminal.backend().to_string());
    }

    #[test]
    fn committed_prompt_is_written_above_the_viewport_and_draft_line_shows_cursor() {
        let mut terminal = test_terminal(56, 10);
        let mut app = app();
        app.draft.push_str("跑一次");
        paint(&mut terminal, &mut app).expect("paint first frame");
        app.on_key(key(KeyCode::Enter, KeyModifiers::NONE));
        paint(&mut terminal, &mut app).expect("paint after submit");
        assert!(app.pending.is_empty(), "committed cells must drain");
        let rendered = terminal.backend().to_string();
        assert!(
            rendered.contains("跑一次"),
            "committed prompt missing from the frame:\n{rendered}"
        );
        insta::assert_snapshot!(terminal.backend().to_string());
    }
}
