//! H-258: exercise the real app event route, tail, and terminal scrollback.
use super::*;
use crate::tui::state::retry_audit::{observe, FirstAttempt};
use deepagent_runtime::RuntimeEvent;
use ratatui::widgets::Widget;
use ratatui::{backend::TestBackend, buffer::Buffer, Terminal, TerminalOptions, Viewport};

fn buffer_text(buffer: &Buffer) -> String {
    let mut text = String::new();
    if buffer.area.width == 0 {
        return text;
    }
    for row in buffer.content.chunks(buffer.area.width as usize) {
        let mut skip = 0;
        for cell in row {
            if skip > 0 {
                skip -= 1;
            } else {
                text.push_str(cell.symbol());
                skip = display_width(cell.symbol()).saturating_sub(1);
            }
        }
        text.push('\n');
    }
    text
}

#[tokio::test]
async fn h258_production_events_rewind_the_live_tail_without_polluting_terminal_history() {
    let observation = observe(FirstAttempt::DisconnectAfterDelta).await;
    let mut terminal = Terminal::with_options(
        TestBackend::new(100, 8),
        TerminalOptions {
            viewport: Viewport::Inline(VIEWPORT_ROWS),
        },
    )
    .unwrap();
    let mut app = super::tests::test_app();
    terminal.draw(|frame| render(frame, &app)).unwrap();
    let mut flushed = Vec::new();
    for event in observation.runtime {
        let reset = matches!(event, RuntimeEvent::ModelAttemptReset { .. });
        app.on_ui_event(UiEvent::Runtime(event));
        app.collect_flushable();
        let cells = app.take_flushable();
        assert!(!format!("{cells:?}").contains("已废弃"));
        for cell in &cells {
            let lines = cell.to_lines(100);
            let height = lines.len() as u16;
            let paragraph = ratatui::widgets::Paragraph::new(ratatui::text::Text::from(lines));
            terminal
                .insert_before(height, |buffer| paragraph.render(buffer.area, buffer))
                .unwrap();
        }
        flushed.extend(cells);
        terminal.draw(|frame| render(frame, &app)).unwrap();
        if reset {
            assert!(!buffer_text(terminal.backend().buffer()).contains("已废弃"));
        }
    }
    let actual = format!("{:?}", flushed);
    assert!(actual.contains("有效答案：乙。"));
    assert!(!actual.contains("已废弃"));
    let screen = buffer_text(terminal.backend().buffer());
    let scrollback = buffer_text(terminal.backend().scrollback());
    assert!(!screen.contains("已废弃"));
    assert!(!scrollback.contains("已废弃"));
    assert!(format!("{screen}{scrollback}").contains("有效答案：乙。"));
}

/// Explicit acceptance only: the ordinary test harness is redirected, not a TTY.
#[tokio::test]
#[ignore = "H-258 real TTY acceptance: run explicitly in an isolated pseudo console"]
async fn h258_real_tty_replays_production_retry_through_the_actual_flush_and_paint() {
    use std::io::IsTerminal;
    assert!(std::io::stdin().is_terminal() && std::io::stdout().is_terminal());
    let observation = observe(FirstAttempt::DisconnectAfterDelta).await;
    let mut app = super::tests::test_app();
    let mut guard = TerminalGuard::inline(VIEWPORT_ROWS).unwrap();
    let mut reset_seen = false;
    let mut transcript = Vec::new();
    for event in observation.runtime {
        let reset = matches!(event, RuntimeEvent::ModelAttemptReset { .. });
        app.on_ui_event(UiEvent::Runtime(event));
        app.collect_flushable();
        let cells = app.take_flushable();
        assert!(!format!("{cells:?}").contains("已废弃"));
        flush(&mut guard, &cells).unwrap();
        paint(&mut guard, &app).unwrap();
        transcript.extend(cells);
        if reset {
            reset_seen = true;
            let line = app.tail_line(100);
            assert!(!line
                .spans
                .iter()
                .any(|span| span.content.contains("已废弃")));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(guard);
    assert!(reset_seen);
    assert!(!format!("{transcript:?}").contains("已废弃"));
    assert!(format!("{transcript:?}").contains("有效答案：乙。"));
    println!("H258_REAL_TTY_ACCEPTED: reset, actual paint/flush, committed transcript agree");
}
