//! Bounded forwarding jobs owned by one authenticated transport, never replayed on reconnect.
use crate::advanced::extension_error;
use crate::connection_lease::ResourceLease;
use crate::extensions::ConnectionExtensions;
use crate::{SshClient, SshResult};
use russh::{
    client::{ChannelOpenHandle, Msg},
    Channel, ChannelOpenFailure,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_JOBS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshForwardKind {
    Local,
    Remote,
    Dynamic,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshForwardRequest {
    pub kind: SshForwardKind,
    pub bind_host: String,
    pub bind_port: u16,
    pub target_host: Option<String>,
    pub target_port: Option<u16>,
    #[serde(default)]
    pub allow_non_loopback: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshForwardStatus {
    Starting,
    Running,
    Stopped,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshForwardTask {
    pub id: String,
    pub connection_id: String,
    pub transport_generation: u64,
    pub kind: SshForwardKind,
    pub bind_host: String,
    pub bind_port: u16,
    pub target_host: Option<String>,
    pub target_port: Option<u16>,
    pub status: SshForwardStatus,
    pub active_channels: u32,
    pub completed_channels: u64,
    pub bytes_to_target: u64,
    pub bytes_from_target: u64,
    pub events: Vec<String>,
}

pub(crate) struct ForwardJob {
    state: Mutex<SshForwardTask>,
    pub cancelled: CancellationToken,
    pub tasks: TaskTracker,
    allocated: watch::Sender<Option<u16>>,
    allocation_in_flight: AtomicBool,
    stop: tokio::sync::Mutex<()>,
    resource: Mutex<Option<ResourceLease>>,
}

impl ForwardJob {
    fn snapshot(&self) -> SshForwardTask {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn event(&self, message: impl Into<String>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.events.len() == 32 {
            state.events.remove(0);
        }
        state
            .events
            .push(message.into().chars().take(512).collect());
    }
    fn finish(&self, status: SshForwardStatus) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).status = status;
        self.resource
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }
    fn start_channel(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active_channels += 1;
    }
    fn end_channel(&self, result: std::io::Result<(u64, u64)>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.completed_channels = state.completed_channels.saturating_add(1);
        if let Ok((outgoing, incoming)) = result {
            state.bytes_to_target = state.bytes_to_target.saturating_add(outgoing);
            state.bytes_from_target = state.bytes_from_target.saturating_add(incoming);
        }
    }
}

#[derive(Default)]
pub(crate) struct ForwardRegistry {
    jobs: Mutex<HashMap<String, Arc<ForwardJob>>>,
    remote_start: tokio::sync::Mutex<()>,
}

impl ForwardRegistry {
    pub async fn wait(&self) {
        let jobs: Vec<_> = self
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        for job in jobs {
            job.tasks.wait().await;
        }
    }
    pub fn disconnect(&self) {
        for job in self.jobs.lock().unwrap_or_else(|e| e.into_inner()).values() {
            job.cancelled.cancel();
            job.tasks.close();
            job.finish(SshForwardStatus::Stopped);
        }
    }

    fn insert(&self, job: Arc<ForwardJob>) -> SshResult<()> {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if jobs.len() >= MAX_JOBS {
            jobs.retain(|_, entry| {
                !entry.cancelled.is_cancelled()
                    && matches!(
                        entry.snapshot().status,
                        SshForwardStatus::Starting | SshForwardStatus::Running
                    )
            });
        }
        if jobs.len() >= MAX_JOBS {
            return Err(extension_error("start", "forwarding task limit reached"));
        }
        jobs.insert(job.snapshot().id, job);
        Ok(())
    }

    pub fn list(&self) -> Vec<SshForwardTask> {
        let mut result: Vec<_> = self
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|j| j.snapshot())
            .collect();
        result.sort_by(|a, b| a.id.cmp(&b.id));
        result
    }

    fn remote_job(&self, address: &str, port: u32) -> Option<Arc<ForwardJob>> {
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let eligible = |job: &&Arc<ForwardJob>| {
            let state = job.snapshot();
            state.kind == SshForwardKind::Remote
                && !job.cancelled.is_cancelled()
                && state.bind_host == address
                && matches!(
                    state.status,
                    SshForwardStatus::Starting | SshForwardStatus::Running
                )
        };
        // Prefer an established listener. Only the request currently on the wire may
        // receive a channel before the server's allocation reply has been processed.
        jobs.values()
            .filter(eligible)
            .find(|job| {
                job.allocated
                    .borrow()
                    .is_some_and(|value| u32::from(value) == port)
            })
            .or_else(|| {
                jobs.values().filter(eligible).find(|job| {
                    let state = job.snapshot();
                    job.allocation_in_flight.load(Ordering::Acquire)
                        && (state.bind_port == 0 || u32::from(state.bind_port) == port)
                })
            })
            .cloned()
    }
}

fn host_valid(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '/' | '\\' | '\0'))
}

impl SshForwardRequest {
    fn validate(&self) -> SshResult<IpAddr> {
        let address: IpAddr = self.bind_host.parse().map_err(|_| {
            extension_error(
                "configuration",
                "listener requires an explicit IPv4 or IPv6 address",
            )
        })?;
        if !address.is_loopback() && !self.allow_non_loopback {
            return Err(extension_error(
                "configuration",
                "non-loopback listener requires explicit confirmation",
            ));
        }
        match self.kind {
            SshForwardKind::Dynamic => {
                if self.target_host.is_some() || self.target_port.is_some() {
                    return Err(extension_error(
                        "configuration",
                        "SOCKS chooses its target per connection",
                    ));
                }
            }
            _ => {
                if !self.target_host.as_deref().is_some_and(host_valid)
                    || !self.target_port.is_some_and(|p| p != 0)
                {
                    return Err(extension_error(
                        "configuration",
                        "a valid target host and nonzero port are required",
                    ));
                }
            }
        }
        Ok(address)
    }
}

impl SshClient {
    pub(crate) async fn start_forward(
        &self,
        connection_id: &str,
        generation: u64,
        request: SshForwardRequest,
        resource: ResourceLease,
    ) -> SshResult<SshForwardTask> {
        let address = request.validate()?;
        if self.is_closed() || self.extensions.cancelled.is_cancelled() {
            return Err(extension_error("start", "SSH transport is closed"));
        }
        let job = Arc::new(ForwardJob {
            state: Mutex::new(SshForwardTask {
                id: uuid::Uuid::new_v4().to_string(),
                connection_id: connection_id.into(),
                transport_generation: generation,
                kind: request.kind,
                bind_host: request.bind_host.clone(),
                bind_port: request.bind_port,
                target_host: request.target_host,
                target_port: request.target_port,
                status: SshForwardStatus::Starting,
                active_channels: 0,
                completed_channels: 0,
                bytes_to_target: 0,
                bytes_from_target: 0,
                events: vec!["starting".into()],
            }),
            cancelled: self.extensions.cancelled.child_token(),
            tasks: TaskTracker::new(),
            allocated: watch::channel(None).0,
            allocation_in_flight: AtomicBool::new(false),
            stop: tokio::sync::Mutex::new(()),
            resource: Mutex::new(Some(resource)),
        });
        self.extensions.forwards.insert(job.clone())?;
        // Cancellation of the caller also cancels an incompletely published local listener.
        let mut opening = OpeningForward {
            job: job.clone(),
            published: false,
            client: self.clone(),
            unknown_remote: false,
        };
        let result = if request.kind == SshForwardKind::Remote {
            let _serial = self.extensions.forwards.remote_start.lock().await;
            let state = job.snapshot();
            if job.cancelled.is_cancelled() {
                return Err(extension_error(
                    "remote allocation",
                    "allocation cancelled before request",
                ));
            }
            opening.unknown_remote = true;
            job.allocation_in_flight.store(true, Ordering::Release);
            // A cancelled global request may have succeeded remotely without its reply.
            // Revoke the whole exact transport if the allocation outcome is unknown.
            let reply = tokio::select! {
                biased;
                _=job.cancelled.cancelled()=>Err(extension_error("remote allocation","allocation cancelled; disconnect before retrying")),
                result=tokio::time::timeout(CONNECT_TIMEOUT,self.handle.tcpip_forward(state.bind_host.clone(),u32::from(state.bind_port)))=> {
                    match result {
                        Ok(Ok(port)) => { Ok(if state.bind_port==0 { port } else { u32::from(state.bind_port) }) },
                        Ok(Err(error)) => { opening.unknown_remote=false; Err(extension_error("remote allocation",format!("server rejected remote listener: {error}"))) },
                        Err(_) => { let _=self.disconnect().await; Err(extension_error("remote allocation","allocation timed out; transport closed because listener outcome is unknown")) }
                    }
                }
            };
            match reply {
                Ok(port) if port > 0 && port <= u32::from(u16::MAX) => {
                    job.state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .bind_port = port as u16;
                    job.allocated.send_replace(Some(port as u16));
                    job.allocation_in_flight.store(false, Ordering::Release);
                    Ok(())
                }
                Ok(_) => {
                    let _ = self.disconnect().await;
                    Err(extension_error(
                        "remote allocation",
                        "server returned an invalid allocated port",
                    ))
                }
                Err(error) => {
                    job.allocation_in_flight.store(false, Ordering::Release);
                    Err(error)
                }
            }
        } else {
            match TcpListener::bind((address, request.bind_port)).await {
                Ok(listener) => {
                    let port = listener.local_addr()?.port();
                    job.state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .bind_port = port;
                    job.allocated.send_replace(Some(port));
                    let client = self.clone();
                    let actor = job.clone();
                    job.tasks.spawn(async move {
                        client.serve_listener(listener, actor).await;
                    });
                    Ok(())
                }
                Err(error) => Err(extension_error("bind", error.to_string())),
            }
        };
        if let Err(error) = result {
            job.event(error.to_string());
            job.finish(SshForwardStatus::Error);
            return Err(error);
        }
        if job.cancelled.is_cancelled() {
            return Err(extension_error(
                "start",
                "SSH transport was retired while forwarding started",
            ));
        }
        job.state.lock().unwrap_or_else(|e| e.into_inner()).status = SshForwardStatus::Running;
        job.event("running");
        opening.published = true;
        tracing::info!(target:"deepagent_ssh",connection_id,generation,kind=?request.kind,port=job.snapshot().bind_port,"forwarding started");
        Ok(job.snapshot())
    }

    async fn serve_listener(&self, listener: TcpListener, job: Arc<ForwardJob>) {
        loop {
            let accepted = tokio::select! {biased;_=job.cancelled.cancelled()=>break,result=listener.accept()=>result};
            let (socket, origin) = match accepted {
                Ok(value) => value,
                Err(error) => {
                    job.event(format!("accept: {error}"));
                    job.cancelled.cancel();
                    job.finish(SshForwardStatus::Error);
                    break;
                }
            };
            let Ok(permit) = self.extensions.channels.clone().try_acquire_owned() else {
                job.event("channel limit reached");
                drop(socket);
                continue;
            };
            let client = self.clone();
            let stream_job = job.clone();
            job.tasks.spawn(async move {
                let _permit=permit;
                let result=tokio::select! {biased;_=stream_job.cancelled.cancelled()=>Ok(()),result=client.local_channel(socket,origin,stream_job.clone())=>result};
                if let Err(error)=result {stream_job.event(error.to_string());}
            });
        }
    }

    async fn local_channel(
        &self,
        mut socket: TcpStream,
        origin: std::net::SocketAddr,
        job: Arc<ForwardJob>,
    ) -> SshResult<()> {
        let state = job.snapshot();
        let dynamic = state.kind == SshForwardKind::Dynamic;
        let (host, port) = if dynamic {
            tokio::time::timeout(CONNECT_TIMEOUT, socks_target(&mut socket))
                .await
                .map_err(|_| extension_error("SOCKS", "handshake timed out"))??
        } else {
            (state.target_host.unwrap(), state.target_port.unwrap())
        };
        // Domain names are passed to SSH for resolution by the remote endpoint.
        let opened = tokio::time::timeout(
            CONNECT_TIMEOUT,
            self.handle.channel_open_direct_tcpip(
                host,
                u32::from(port),
                origin.ip().to_string(),
                u32::from(origin.port()),
            ),
        )
        .await;
        let channel = match opened {
            Ok(Ok(channel)) => channel,
            _ => {
                if dynamic {
                    let _ = socket.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await;
                }
                return Err(extension_error(
                    "connect",
                    "SSH target refused or timed out",
                ));
            }
        };
        if dynamic {
            socket.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        }
        let mut remote = channel.into_stream();
        job.start_channel();
        let _activity = ChannelActivity(job.clone());
        let result = tokio::select! {biased;_=job.cancelled.cancelled()=>Ok((0,0)),result=tokio::io::copy_bidirectional(&mut socket,&mut remote)=>result};
        if let Err(error) = &result {
            job.event(format!("stream: {error}"));
        }
        job.end_channel(result);
        Ok(())
    }

    pub(crate) async fn stop_forward(&self, id: &str) -> SshResult<SshForwardTask> {
        let job = self
            .extensions
            .forwards
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(|| {
                extension_error("stop", "forwarding task not found on this transport")
            })?;
        let _stop = job.stop.lock().await;
        let state = job.snapshot();
        if state.status == SshForwardStatus::Stopped {
            return Ok(state);
        }
        let allocated = *job.allocated.borrow();
        job.cancelled.cancel();
        job.tasks.close();
        job.tasks.wait().await;
        if state.kind == SshForwardKind::Remote && !self.is_closed() && allocated.is_some() {
            match tokio::time::timeout(
                CONNECT_TIMEOUT,
                self.handle
                    .cancel_tcpip_forward(state.bind_host, u32::from(allocated.unwrap())),
            )
            .await
            {
                Ok(Ok(())) => {}
                _ => {
                    job.event(
                        "server did not confirm listener removal; disconnect to guarantee cleanup",
                    );
                    job.finish(SshForwardStatus::Error);
                    return Err(extension_error("stop","server did not confirm remote listener removal; disconnect to guarantee cleanup"));
                }
            }
        }
        job.finish(SshForwardStatus::Stopped);
        job.event("stopped");
        Ok(job.snapshot())
    }
}

struct OpeningForward {
    job: Arc<ForwardJob>,
    published: bool,
    client: SshClient,
    unknown_remote: bool,
}
impl Drop for OpeningForward {
    fn drop(&mut self) {
        if !self.published {
            self.job.cancelled.cancel();
            self.job.tasks.close();
            self.job
                .allocation_in_flight
                .store(false, Ordering::Release);
            if self.job.snapshot().status == SshForwardStatus::Starting {
                self.job.event("start cancelled before publication");
                self.job.finish(SshForwardStatus::Stopped);
            } else {
                self.job
                    .resource
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take();
            }
            if self.unknown_remote {
                self.job
                    .event("remote allocation outcome unknown; retiring this SSH transport");
                self.client.extensions.stop();
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    let client = self.client.clone();
                    runtime.spawn(async move {
                        let _ = client.disconnect().await;
                    });
                }
            }
        }
    }
}

struct ChannelActivity(Arc<ForwardJob>);
impl Drop for ChannelActivity {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active_channels = state.active_channels.saturating_sub(1);
    }
}

pub(crate) async fn accept_remote(
    extensions: Arc<ConnectionExtensions>,
    channel: Channel<Msg>,
    reply: ChannelOpenHandle,
    address: String,
    port: u32,
) {
    let Some(job) = extensions.forwards.remote_job(&address, port) else {
        reply
            .reject(ChannelOpenFailure::AdministrativelyProhibited)
            .await;
        return;
    };
    let Ok(permit) = extensions.channels.clone().try_acquire_owned() else {
        reply.reject(ChannelOpenFailure::ResourceShortage).await;
        return;
    };
    let actor = job.clone();
    job.tasks.spawn(async move {
        let _permit=permit;
        let cancelled=actor.cancelled.clone();
        tokio::select! {biased;_=cancelled.cancelled()=>{},_=async {
            let mut allocated=actor.allocated.subscribe();
            let allocation=tokio::time::timeout(CONNECT_TIMEOUT,async {
                loop { if let Some(value)=*allocated.borrow_and_update(){break Some(value);} if allocated.changed().await.is_err(){break None;} }
            }).await;
            if !matches!(allocation,Ok(Some(value)) if u32::from(value)==port) {reply.reject(ChannelOpenFailure::AdministrativelyProhibited).await;return;}
            let state=actor.snapshot();
            let connect=tokio::time::timeout(CONNECT_TIMEOUT,TcpStream::connect((state.target_host.as_deref().unwrap(),state.target_port.unwrap()))).await;
            let Ok(Ok(mut local))=connect else {actor.event("local target refused or timed out");reply.reject(ChannelOpenFailure::ConnectFailed).await;return;};
            reply.accept().await;
            let mut remote=channel.into_stream(); actor.start_channel();
            let _activity=ChannelActivity(actor.clone());
            let result=tokio::select! {biased;_=actor.cancelled.cancelled()=>Ok((0,0)),result=tokio::io::copy_bidirectional(&mut remote,&mut local)=>result};
            actor.end_channel(result);
        }=>{}}
    });
}

async fn socks_target(socket: &mut TcpStream) -> SshResult<(String, u16)> {
    if socket.read_u8().await? != 5 {
        return Err(extension_error("SOCKS", "only SOCKS5 is supported"));
    }
    let count = socket.read_u8().await?;
    if count == 0 {
        return Err(extension_error("SOCKS", "no authentication method"));
    }
    let mut methods = vec![0; usize::from(count)];
    socket.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        socket.write_all(&[5, 255]).await?;
        return Err(extension_error(
            "SOCKS",
            "no supported authentication method",
        ));
    }
    socket.write_all(&[5, 0]).await?;
    let mut header = [0; 4];
    socket.read_exact(&mut header).await?;
    if header[0] != 5 || header[1] != 1 || header[2] != 0 {
        socket.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        return Err(extension_error("SOCKS", "only CONNECT is supported"));
    }
    let host = match header[3] {
        1 => {
            let mut ip = [0; 4];
            socket.read_exact(&mut ip).await?;
            std::net::Ipv4Addr::from(ip).to_string()
        }
        4 => {
            let mut ip = [0; 16];
            socket.read_exact(&mut ip).await?;
            std::net::Ipv6Addr::from(ip).to_string()
        }
        3 => {
            let size = socket.read_u8().await?;
            let mut name = vec![0; usize::from(size)];
            socket.read_exact(&mut name).await?;
            String::from_utf8(name).map_err(|_| extension_error("SOCKS", "invalid target name"))?
        }
        _ => {
            socket.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            return Err(extension_error("SOCKS", "unsupported address type"));
        }
    };
    let port = socket.read_u16().await?;
    if !host_valid(&host) || port == 0 {
        return Err(extension_error("SOCKS", "invalid target"));
    }
    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote_job(port: u16) -> Arc<ForwardJob> {
        Arc::new(ForwardJob {
            state: Mutex::new(SshForwardTask {
                id: uuid::Uuid::new_v4().to_string(),
                connection_id: "fixture".into(),
                transport_generation: 1,
                kind: SshForwardKind::Remote,
                bind_host: "127.0.0.1".into(),
                bind_port: port,
                target_host: Some("127.0.0.1".into()),
                target_port: Some(22),
                status: SshForwardStatus::Starting,
                active_channels: 0,
                completed_channels: 0,
                bytes_to_target: 0,
                bytes_from_target: 0,
                events: vec![],
            }),
            cancelled: CancellationToken::new(),
            tasks: TaskTracker::new(),
            allocated: watch::channel(None).0,
            allocation_in_flight: AtomicBool::new(false),
            stop: tokio::sync::Mutex::new(()),
            resource: Mutex::new(None),
        })
    }

    #[test]
    fn remote_channels_match_only_allocated_or_in_flight_requests() {
        let registry = ForwardRegistry::default();
        let established = remote_job(12345);
        established.allocated.send_replace(Some(12345));
        let in_flight = remote_job(0);
        in_flight
            .allocation_in_flight
            .store(true, Ordering::Release);
        let queued = remote_job(0);
        for job in [&established, &in_flight, &queued] {
            registry.insert(job.clone()).unwrap();
        }
        assert!(Arc::ptr_eq(
            &registry.remote_job("127.0.0.1", 12345).unwrap(),
            &established
        ));
        assert!(Arc::ptr_eq(
            &registry.remote_job("127.0.0.1", 12346).unwrap(),
            &in_flight
        ));
        in_flight.cancelled.cancel();
        assert!(registry.remote_job("127.0.0.1", 12346).is_none());
    }

    #[test]
    fn cancelled_starting_jobs_do_not_exhaust_registry_capacity() {
        let registry = ForwardRegistry::default();
        for _ in 0..MAX_JOBS * 2 {
            let job = remote_job(0);
            job.cancelled.cancel();
            registry.insert(job).unwrap();
        }
        registry.insert(remote_job(0)).unwrap();
    }

    #[test]
    fn listener_exposure_and_targets_require_explicit_valid_input() {
        let mut request = SshForwardRequest {
            kind: SshForwardKind::Local,
            bind_host: "0.0.0.0".into(),
            bind_port: 0,
            target_host: Some("fixture.invalid".into()),
            target_port: Some(22),
            allow_non_loopback: false,
        };
        assert!(request.validate().is_err());
        request.allow_non_loopback = true;
        assert!(request.validate().is_ok());
        request.target_host = Some("bad\nname".into());
        assert!(request.validate().is_err());
        request.kind = SshForwardKind::Dynamic;
        request.target_host = None;
        request.target_port = None;
        assert!(request.validate().is_ok());
    }
}
