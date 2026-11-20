//! Acknowledged PTY setup and one channel actor. Connection ownership stays in SshService.
use crate::session::{PtyCommand, PtyState};
use crate::{SshClient, SshError, SshResult};
use russh::{client::Msg, ChannelMsg, ChannelReadHalf, ChannelWriteHalf};
use std::sync::{atomic::AtomicU64, Arc};
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};

struct OpeningChannel(Option<Arc<ChannelWriteHalf<Msg>>>);

impl Drop for OpeningChannel {
    fn drop(&mut self) {
        if let (Some(channel), Ok(runtime)) = (self.0.take(), tokio::runtime::Handle::try_current())
        {
            runtime.spawn(async move {
                let _ = channel.close().await;
            });
        }
    }
}

async fn acknowledged(
    reader: &mut ChannelReadHalf,
    pending: &mut Vec<Vec<u8>>,
    operation: &str,
) -> SshResult<()> {
    let mut buffered = 0;
    loop {
        match reader.wait().await {
            Some(ChannelMsg::Success) => return Ok(()),
            Some(ChannelMsg::Failure) => {
                return Err(SshError::Pty(format!(
                    "server rejected {operation} request"
                )))
            }
            Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                buffered += data.len();
                if buffered > 64 * 1024 {
                    return Err(SshError::Pty(
                        "too much output before shell acknowledgement".into(),
                    ));
                }
                pending.push(data.to_vec());
            }
            Some(ChannelMsg::Close) | None => {
                return Err(SshError::Pty(
                    "channel closed before shell acknowledgement".into(),
                ))
            }
            _ => {}
        }
    }
}

pub(super) async fn open(
    client: &SshClient,
    connection_id: &str,
    cols: u16,
    rows: u16,
    cwd: Option<&str>,
) -> SshResult<PtyState> {
    if cols < 2 || rows == 0 {
        return Err(SshError::Pty("invalid terminal dimensions".into()));
    }
    let channel = client.get_channel().await?;
    let (mut reader, writer) = channel.split();
    let writer = Arc::new(writer);
    let mut opening = OpeningChannel(Some(writer.clone()));
    let mut pending = Vec::new();
    let extension_lease = client.extensions.pty_lease();
    tokio::time::timeout(Duration::from_secs(5), async {
        if client.extensions.options.forward_agent {
            // Check the explicitly selected provider before requesting remote forwarding.
            drop(crate::agent::forwarding_stream(client.extensions.options.agent_provider).await?);
            writer
                .agent_forward(true)
                .await
                .map_err(|error| SshError::Pty(format!("request Agent forwarding: {error}")))?;
            acknowledged(&mut reader, &mut pending, "Agent forwarding").await?;
        }
        if let Some(options) = &client.extensions.options.x11 {
            let mut runtime = client.extensions.x11.lock().await;
            let x11 = match runtime.as_ref() {
                Some(x11) => x11.clone(),
                None => crate::x11::X11Runtime::prepare(options).await?,
            };
            *runtime = Some(x11.clone());
            drop(runtime);
            writer
                .request_x11(
                    true,
                    false,
                    "MIT-MAGIC-COOKIE-1",
                    x11.cookie_hex(),
                    x11.screen(),
                )
                .await
                .map_err(|error| SshError::Pty(format!("request X11 forwarding: {error}")))?;
            acknowledged(&mut reader, &mut pending, "X11 forwarding").await?;
        }
        writer
            .request_pty(true, "xterm-256color", cols.into(), rows.into(), 0, 0, &[])
            .await
            .map_err(|error| SshError::Pty(format!("request PTY: {error}")))?;
        acknowledged(&mut reader, &mut pending, "PTY").await?;
        if let Some(cwd) = cwd {
            let command = format!(
                "cd -- {} && exec \"${{SHELL:-/bin/sh}}\" -l",
                crate::remote_file_command::quote(cwd)
            );
            writer
                .exec(true, command)
                .await
                .map_err(|error| SshError::Pty(format!("open directory shell: {error}")))?;
        } else {
            writer
                .request_shell(true)
                .await
                .map_err(|error| SshError::Pty(format!("request shell: {error}")))?;
        }
        acknowledged(&mut reader, &mut pending, "shell").await
    })
    .await
    .map_err(|_| SshError::Pty("PTY/shell acknowledgement exceeded 5 seconds".into()))??;
    let (stdin, mut input) = mpsc::channel::<Vec<u8>>(128);
    let (output, stdout) = mpsc::channel::<Vec<u8>>(256);
    let (commands, mut command) = mpsc::unbounded_channel::<PtyCommand>();
    let output_signal = deepagent_terminal::TerminalOutputSignal::default();
    let activity = output_signal.clone();
    let channel = writer.clone();
    let connection_id = connection_id.to_owned();
    let join = tokio::spawn(async move {
        let _extension_lease = extension_lease;
        for bytes in pending {
            if output.send(bytes).await.is_err() {
                let _ = writer.close().await;
                return;
            }
            activity.notify();
        }
        loop {
            tokio::select! {
                message = reader.wait() => match message {
                    Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                        if output.send(data.to_vec()).await.is_err() { break; }
                        activity.notify();
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => tracing::debug!(target:"deepagent_ssh", connection_id, exit_status, "remote shell exited"),
                    Some(ChannelMsg::Close) | None => break,
                    _ => {}
                },
                bytes = input.recv() => match bytes {
                    Some(bytes) if !bytes.is_empty() => { if writer.data(&bytes[..]).await.is_err() { break; } }
                    Some(_) => {}
                    None => break,
                },
                request = command.recv() => match request {
                    Some(PtyCommand::Resize { cols, rows }) => {
                        if writer.window_change(cols.into(), rows.into(), 0, 0).await.is_err() { break; }
                    }
                    None => break,
                },
            }
        }
        // Publish EOF only after dropping the byte producer. A wakeup cannot
        // race ahead of the receiver's closed state and strand its final read.
        drop(output);
        activity.notify();
        let _ = writer.close().await;
        tracing::debug!(target:"deepagent_ssh", connection_id, "SSH PTY actor ended");
    });
    let state = PtyState {
        shared_owner: false,
        _resource: None,
        output_signal,
        stdin,
        commands,
        stdout: Mutex::new(stdout),
        join,
        channel: Some(channel),
        cols,
        rows,
        output_cursor: AtomicU64::new(0),
        history: Mutex::new(Default::default()),
        history_bytes: Mutex::new(0),
    };
    opening.0 = None;
    Ok(state)
}
