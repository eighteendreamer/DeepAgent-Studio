use ratatui::{DefaultTerminal, TerminalOptions, Viewport};

/// Owns the terminal for one TUI session and always hands the shell back a usable
/// terminal, including when the session exits early.
///
/// `ratatui::try_init_with_options` enables raw mode and installs its own panic hook;
/// the inline viewport deliberately skips the alternate screen so committed transcript
/// lines can live in the terminal's real scrollback.
pub struct TerminalGuard {
    terminal: Option<DefaultTerminal>,
}

impl TerminalGuard {
    pub fn inline(rows: u16) -> Result<Self, String> {
        let terminal = ratatui::try_init_with_options(TerminalOptions {
            viewport: Viewport::Inline(rows),
        })
        .map_err(|error| format!("initialize terminal: {error}"))?;
        Ok(Self {
            terminal: Some(terminal),
        })
    }

    pub fn terminal(&mut self) -> &mut DefaultTerminal {
        self.terminal
            .as_mut()
            .expect("terminal guard used after it was restored")
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.terminal.take().is_some() {
            if let Err(error) = ratatui::try_restore() {
                eprintln!("failed to restore terminal: {error}");
            }
        }
    }
}
