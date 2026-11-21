//! Per-transport extension state. Its cancellation never applies to a replacement connection.
use crate::{SshAdvancedOptions, SshNegotiatedAlgorithms};
use russh::{
    client::{ChannelOpenHandle, Msg},
    Channel, ChannelOpenFailure,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConnectionDetails {
    pub transport_generation: u64,
    pub algorithms: Option<SshNegotiatedAlgorithms>,
    pub agent_forwarding_enabled: bool,
    pub x11_forwarding_enabled: bool,
}

pub(crate) struct ConnectionExtensions {
    pub options: SshAdvancedOptions,
    pub algorithms: Mutex<Option<SshNegotiatedAlgorithms>>,
    pub cancelled: CancellationToken,
    pub tasks: TaskTracker,
    pub channels: Arc<Semaphore>,
    pub forwards: crate::forwarding::ForwardRegistry,
    pub x11: tokio::sync::Mutex<Option<Arc<crate::x11::X11Runtime>>>,
    ptys: Mutex<PtyExtensions>,
}

struct PtyExtensions {
    users: usize,
    cancelled: CancellationToken,
}
pub(crate) struct PtyExtensionLease {
    owner: std::sync::Weak<ConnectionExtensions>,
}

impl Drop for PtyExtensionLease {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            let mut ptys = owner.ptys.lock().unwrap_or_else(|e| e.into_inner());
            ptys.users = ptys.users.saturating_sub(1);
            if ptys.users == 0 {
                ptys.cancelled.cancel();
            }
        }
    }
}

impl ConnectionExtensions {
    pub fn new(options: SshAdvancedOptions) -> Arc<Self> {
        Arc::new(Self {
            options,
            algorithms: Mutex::new(None),
            cancelled: CancellationToken::new(),
            tasks: TaskTracker::new(),
            channels: Arc::new(Semaphore::new(32)),
            forwards: Default::default(),
            x11: tokio::sync::Mutex::new(None),
            ptys: Mutex::new(PtyExtensions {
                users: 0,
                cancelled: CancellationToken::new(),
            }),
        })
    }

    pub fn stop(&self) {
        self.cancelled.cancel();
        self.forwards.disconnect();
        self.tasks.close();
    }

    pub async fn shutdown(&self) {
        self.stop();
        self.tasks.wait().await;
        self.forwards.wait().await;
    }

    pub fn pty_lease(self: &Arc<Self>) -> PtyExtensionLease {
        let mut ptys = self.ptys.lock().unwrap_or_else(|e| e.into_inner());
        if ptys.users == 0 {
            ptys.cancelled = self.cancelled.child_token();
        }
        ptys.users += 1;
        PtyExtensionLease {
            owner: Arc::downgrade(self),
        }
    }

    fn pty_cancel(&self) -> Option<CancellationToken> {
        let ptys = self.ptys.lock().unwrap_or_else(|e| e.into_inner());
        (ptys.users != 0 && !self.cancelled.is_cancelled()).then(|| ptys.cancelled.clone())
    }

    pub async fn accept_agent(&self, channel: Channel<Msg>, reply: ChannelOpenHandle) {
        let Some(cancel) = self.pty_cancel().filter(|_| self.options.forward_agent) else {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return;
        };
        let Ok(permit) = self.channels.clone().try_acquire_owned() else {
            reply.reject(ChannelOpenFailure::ResourceShortage).await;
            return;
        };
        let provider = self.options.agent_provider;
        self.tasks.spawn(async move {
            let _permit=permit;
            tokio::select! {biased;_=cancel.cancelled()=>{},_=async {
                let Ok(mut local)=crate::agent::forwarding_stream(provider).await else {reply.reject(ChannelOpenFailure::ConnectFailed).await;return;};
                reply.accept().await;
                let mut remote=channel.into_stream();
                if let Err(error)=crate::agent::bridge_agent(&mut remote,&mut local).await {
                    tracing::debug!(target:"deepagent_ssh",%error,"forwarded Agent channel closed");
                }
            }=>{}}
        });
    }

    pub async fn accept_x11(&self, channel: Channel<Msg>, reply: ChannelOpenHandle) {
        let Some(cancel) = self.pty_cancel().filter(|_| self.options.x11.is_some()) else {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return;
        };
        let Some(x11) = self.x11.lock().await.clone() else {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return;
        };
        let Ok(permit) = self.channels.clone().try_acquire_owned() else {
            reply.reject(ChannelOpenFailure::ResourceShortage).await;
            return;
        };
        self.tasks.spawn(async move {
            let _permit=permit;
            tokio::select! {biased;_=cancel.cancelled()=>{},_=async {
                reply.accept().await;
                if let Err(error)=x11.bridge(channel).await {tracing::debug!(target:"deepagent_ssh",%error,"X11 channel closed");}
            }=>{}}
        });
    }
}

impl Drop for ConnectionExtensions {
    fn drop(&mut self) {
        self.stop();
    }
}
