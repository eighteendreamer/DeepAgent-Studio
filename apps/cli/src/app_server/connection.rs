//! Stdio connection IO and bounded shutdown over the existing executor.
use super::*;

type ConnectionWorkers = FuturesUnordered<tokio::task::JoinHandle<()>>;

pub(super) async fn run_stdio_with_io<R, W>(
    chat: ChatService,
    workspace: PathBuf,
    capabilities: SandboxCapabilities,
    stdin: R,
    stdout: W,
) -> Result<(), String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let stdout = Arc::new(tokio::sync::Mutex::new(stdout));
    let (outbox, mut writer) = EventOutbox::new(stdout.clone());
    let output = outbox.output.clone();
    let mut connection = ServerState::new(chat, workspace, capabilities);
    connection.output = output.clone();
    let state = Arc::new(Mutex::new(connection));
    let emitter = {
        let outbox = outbox.clone();
        Arc::new(move |event: HarnessEvent| {
            outbox.emit(event);
        }) as EventEmitter
    };

    let mut workers = ConnectionWorkers::new();
    let result = serve_requests(&state, &emitter, &output, &stdout, stdin, &mut workers).await;
    tracing::debug!(
        pending_workers = workers.len(),
        "app-server input ended; shutting down connection"
    );
    let mut errors: Vec<_> = result.err().into_iter().collect();
    if let Some(error) = errors.first() {
        output.fail(error.clone());
    }
    // EOF and IO failure share the same owner and cancellation tree. Keep the
    // existing ten-second budget for the whole shutdown, including outbox flush.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    if let Err(cleanup) = cancel_connection_turns(&state, &mut workers, deadline).await {
        let reason = format!("connection cleanup: {cleanup}");
        output.fail(reason.clone());
        errors.push(reason);
    }
    tracing::debug!(
        pending_workers = workers.len(),
        "app-server finished waiting for turn workers"
    );
    // Workers have finished (or shutdown is explicitly unresolved). Release
    // the connection's senders so the sole writer can drain then close naturally.
    drop(emitter);
    drop(outbox);
    match tokio::time::timeout_at(deadline, &mut writer).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => output.fail(format!("app-server event writer failed: {error}")),
        Err(_) => {
            output.fail("event outbox did not drain within the 10s shutdown budget".into());
            writer.abort();
        }
    }
    if let Some(reason) = output.failure_reason() {
        if !errors.contains(&reason) {
            errors.push(reason);
        }
    }
    tracing::debug!(
        failed = !errors.is_empty(),
        "app-server connection shutdown finished"
    );
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

async fn serve_requests<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    state: &Arc<Mutex<ServerState>>,
    emitter: &EventEmitter,
    output: &OutputState,
    stdout: &Arc<tokio::sync::Mutex<W>>,
    stdin: R,
    workers: &mut ConnectionWorkers,
) -> Result<(), String> {
    let mut lines = tokio::io::BufReader::new(stdin).lines();
    loop {
        let line = tokio::select! {
            biased;
            reason = output.failed() => return Err(reason),
            done = workers.next(), if !workers.is_empty() => {
                if let Some(Err(error)) = done {
                    return Err(format!("app-server turn task failed: {error}"));
                }
                continue;
            },
            line = lines.next_line() => line.map_err(|error| format!("read app-server stdin: {error}"))?,
        };
        let Some(line) = line else {
            return Ok(());
        };
        if line.trim().is_empty() {
            continue;
        }
        let request = match serde_json::from_str::<RpcRequest>(&line) {
            Ok(request) => request,
            Err(error) => {
                output
                    .while_healthy(write_response(
                        stdout,
                        &RpcResponse::error(serde_json::Value::Null, -32700, error.to_string()),
                    ))
                    .await?;
                continue;
            }
        };
        // Synchronize ACK validation with the writer's post-flush publication.
        // This also prevents a fast reader racing the high-water update.
        if request.method == "event/ack" {
            output
                .while_healthy(async {
                    let _barrier = stdout.lock().await;
                    Ok(())
                })
                .await?;
        }
        let (response, launch) = {
            let mut guard = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.dispatch(request, Some(emitter))
        };
        let written = output
            .while_healthy(write_response(stdout, &response))
            .await;
        if let Some(launch) = launch {
            workers.push(
                complete_turn_dispatch(state.clone(), emitter.clone(), launch, written).await?,
            );
        } else {
            written?;
        }
    }
}

async fn cancel_connection_turns(
    state: &Arc<Mutex<ServerState>>,
    workers: &mut ConnectionWorkers,
    deadline: tokio::time::Instant,
) -> Result<(), String> {
    // Closed connections must not keep approval waits or queued turns
    // alive. Use their existing cancellation aliases and executor, not a
    // fabricated terminal or another run store.
    let cancellations: Vec<_> = state
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .turns
        .iter()
        .filter(|(_, turn)| turn.active)
        .map(|(id, turn)| (id.clone(), turn.chat.clone()))
        .collect();
    let mut cleanup_errors = Vec::new();
    for (id, chat) in cancellations {
        if let Err(reason) = chat.request_cancel(&id) {
            cleanup_errors.push(format!("cancel {id}: {reason}"));
        }
    }
    // Local bounded shutdown policy. A timeout is reported as unresolved,
    // never converted into success or a synthetic durable terminal.
    let drained = tokio::time::timeout_at(deadline, async {
        while let Some(done) = workers.next().await {
            if let Err(reason) = done {
                cleanup_errors.push(format!("finalize turn: {reason}"));
            }
        }
    })
    .await;
    if drained.is_err() {
        cleanup_errors.push("turn cancellation did not finish within 10s".into());
    }
    if cleanup_errors.is_empty() {
        Ok(())
    } else {
        Err(cleanup_errors.join("; "))
    }
}

pub(super) async fn complete_turn_dispatch(
    state: Arc<Mutex<ServerState>>,
    emitter: EventEmitter,
    launch: TurnLaunch,
    written: Result<(), String>,
) -> Result<tokio::task::JoinHandle<()>, String> {
    // Admission already committed. If its ACK cannot be written, cancel before
    // preparation and let the same executor finalize it before returning the
    // transport error to the host.
    let cancellation = written
        .is_err()
        .then(|| launch.chat.request_cancel(&launch.turn_id));
    let worker = spawn_turn(state, emitter, launch);
    if let Some(cancellation) = cancellation {
        worker
            .await
            .map_err(|error| format!("finalize undelivered turn ACK: {error}"))?;
        cancellation.map_err(|error| error.to_string())?;
        return Err(written.unwrap_err());
    }
    Ok(worker)
}
