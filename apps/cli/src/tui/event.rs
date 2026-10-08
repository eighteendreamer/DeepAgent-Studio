use crossterm::event::{Event, EventStream, KeyEvent, KeyEventKind};
use futures::StreamExt;
use tokio::sync::mpsc;

/// Input the UI loop reacts to. Drawing is requested by the loop itself, so it is
/// deliberately not a variant here.
#[derive(Debug)]
pub enum TuiEvent {
    Key(KeyEvent),
    Paste(String),
    /// Carries no dimensions: `Terminal::draw` asks the backend for the new size,
    /// so the loop only needs to know that a repaint is due.
    Resize,
}

/// Bridges crossterm's async event source into a channel the UI loop can select on.
///
/// The sender is dropped when the terminal read fails, which the loop treats as
/// "the terminal went away" instead of spinning on a dead descriptor.
pub fn spawn_pump(sender: mpsc::Sender<TuiEvent>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut stream = EventStream::new();
        while let Some(next) = stream.next().await {
            let event = match next {
                Ok(event) => event,
                Err(error) => {
                    tracing::debug!("terminal event stream ended: {error}");
                    break;
                }
            };
            let translated = match event {
                // Windows delivers press and release for the same key stroke; acting on
                // both would double every character.
                Event::Key(key) if key.kind == KeyEventKind::Press => Some(TuiEvent::Key(key)),
                Event::Paste(text) => Some(TuiEvent::Paste(text)),
                Event::Resize(width, height) => {
                    // The dimensions themselves are not needed: Terminal::draw
                    // re-reads the backend size, so this only has to wake the loop.
                    tracing::trace!("terminal resized to {width}x{height}");
                    Some(TuiEvent::Resize)
                }
                _ => None,
            };
            if let Some(translated) = translated {
                if sender.send(translated).await.is_err() {
                    break;
                }
            }
        }
    })
}
