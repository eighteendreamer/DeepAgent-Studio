//! Bounded exec for reviewed file-operation helpers. No credentials enter argv.
use crate::{FileTransferError, SshService};
use russh::ChannelMsg;
use tokio_util::sync::CancellationToken;

pub(super) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) async fn run(
    ssh: &SshService,
    id: &str,
    generation: Option<u64>,
    command: &str,
    cancel: &CancellationToken,
) -> Result<String, FileTransferError> {
    let (owner, client) = crate::file_transfer::operation(
        cancel,
        ssh.inner.file_client_for_generation(id, generation),
    )
    .await?;
    let mut channel = crate::file_transfer::operation(cancel, client.get_channel()).await?;
    let work = async {
        crate::file_transfer::operation(cancel, channel.exec(true, command)).await?;
        let mut output = Vec::new();
        let mut stderr = Vec::new();
        let mut status = None;
        while let Some(message) = channel.wait().await {
            match message {
                ChannelMsg::Data { data } => {
                    if output.len() + data.len() > 8 * 1024 * 1024 {
                        return Err(fail("limit", "file helper output exceeds 8 MiB"));
                    }
                    output.extend_from_slice(&data);
                }
                ChannelMsg::ExtendedData { data, ext: 1 } => {
                    let n = data.len().min(4096_usize.saturating_sub(stderr.len()));
                    stderr.extend_from_slice(&data[..n]);
                }
                ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                ChannelMsg::Failure => {
                    return Err(fail("unsupported", "remote file helper was rejected"))
                }
                _ => {}
            }
        }
        if status != Some(0) {
            return Err(fail(
                "remote",
                &format!(
                    "remote file helper exited {:?}: {}",
                    status,
                    String::from_utf8_lossy(&stderr)
                ),
            ));
        }
        String::from_utf8(output)
            .map_err(|_| fail("invalid", "file helper returned non-UTF8 output"))
    };
    let result = tokio::select! {
        biased;
        _=cancel.cancelled()=>Err(fail("cancelled","file operation stopped; review destination for partial effects")),
        result=tokio::time::timeout(std::time::Duration::from_secs(180),work)=>match result { Ok(value)=>value,Err(_)=>Err(fail("timeout","file operation timed out; review destination for partial effects")) },
    };
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), channel.close()).await;
    if result.is_ok() {
        owner.touch_keepalive();
    }
    result
}

fn fail(code: &str, message: &str) -> FileTransferError {
    FileTransferError::new(code, message)
}
