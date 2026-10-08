//! Interactive terminal agent.
//!
//! A real terminal gets the ratatui interface in [`app`]; anything else (CI smoke
//! test, pipes, `deepagent | tee`) keeps the historical line-mode loop in
//! [`linemode`] so the existing callers of `deepagent chat` do not break.

mod app;
mod event;
mod history;
mod linemode;
mod questions;
mod terminal;

use std::io::IsTerminal;

/// Runs one interactive session until the user quits.
pub async fn run(options: crate::args::RunOptions) -> Result<(), String> {
    if use_linemode(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    ) {
        return linemode::run(options).await;
    }
    app::run(options).await
}

/// Line mode is the fallback for any stream that is not an interactive terminal:
/// ratatui needs both a keyboard and a screen it owns.
fn use_linemode(stdin_terminal: bool, stdout_terminal: bool) -> bool {
    !(stdin_terminal && stdout_terminal)
}

#[cfg(test)]
mod tests {
    use super::use_linemode;

    #[test]
    fn only_a_full_terminal_gets_the_tui() {
        assert!(!use_linemode(true, true));
        assert!(use_linemode(false, true));
        assert!(use_linemode(true, false));
        assert!(use_linemode(false, false));
    }
}
