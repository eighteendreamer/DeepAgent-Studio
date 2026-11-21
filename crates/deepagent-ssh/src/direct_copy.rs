//! Direct transfers use source-side credentials and a pinned destination public key.
//! They never export a local private key or enable Agent forwarding.
use crate::remote_file_command::{quote, run};
use crate::{FileTransferError, ServerCopyMethod, ServerCopyRequest, SshService};
use russh::keys::{HashAlg, PublicKey};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectCopyProbe {
    pub source_connection_id: String,
    pub destination_connection_id: String,
    pub scp: bool,
    pub rsync: bool,
    pub authenticated: bool,
    pub destination_fingerprint: Option<String>,
    pub reason: Option<String>,
}
pub(super) struct DirectCopyCapability {
    host: String,
    port: u16,
    user: String,
    known_host: String,
    source_generation: u64,
}
impl SshService {
    pub async fn probe_direct_copy(
        &self,
        source_id: &str,
        destination_id: &str,
        cancel: CancellationToken,
    ) -> DirectCopyProbe {
        let mut probe = DirectCopyProbe {
            source_connection_id: source_id.into(),
            destination_connection_id: destination_id.into(),
            scp: false,
            rsync: false,
            authenticated: false,
            destination_fingerprint: None,
            reason: None,
        };
        match self
            .direct_capability(source_id, destination_id, None, None, &cancel)
            .await
        {
            Ok((cap, scp, rsync, fingerprint)) => {
                probe.scp = scp;
                probe.rsync = rsync;
                probe.destination_fingerprint = Some(fingerprint);
                match self
                    .authenticate_direct_copy(source_id, &cap, &cancel)
                    .await
                {
                    Ok(_) => probe.authenticated = true,
                    Err(error) => probe.reason = Some(error.message),
                }
            }
            Err(error) => probe.reason = Some(error.message),
        }
        probe
    }
    pub(super) async fn require_direct_copy(
        &self,
        request: &ServerCopyRequest,
        cancel: &CancellationToken,
    ) -> Result<DirectCopyCapability, FileTransferError> {
        let (sg, dg) = request.validate()?;
        let (cap, scp, rsync, _) = self
            .direct_capability(
                &request.spec.source_connection_id,
                &request.spec.destination_connection_id,
                Some(sg),
                Some(dg),
                cancel,
            )
            .await?;
        if (request.spec.method == ServerCopyMethod::Scp && !scp)
            || (request.spec.method == ServerCopyMethod::Rsync && !rsync)
        {
            return Err(fail(
                "direct copy tool unavailable; preview and approve relay instead",
            ));
        }
        self.authenticate_direct_copy(&request.spec.source_connection_id, &cap, cancel)
            .await?;
        Ok(cap)
    }
    async fn authenticate_direct_copy(
        &self,
        source_id: &str,
        cap: &DirectCopyCapability,
        cancel: &CancellationToken,
    ) -> Result<(), FileTransferError> {
        let command = with_trust(
            cap,
            &format!("ssh {} -- {} true", ssh_options(cap), quote(&cap.host)),
        );
        run(
            self,
            source_id,
            Some(cap.source_generation),
            &command,
            cancel,
        )
        .await
        .map(|_| ())
        .map_err(|mut error| {
            if error.code == "remote" {
                error.message = format!(
                    "source-side noninteractive authentication unavailable; choose relay: {}",
                    error.message
                );
            }
            error
        })
    }
    async fn direct_capability(
        &self,
        source_id: &str,
        destination_id: &str,
        sg: Option<u64>,
        dg: Option<u64>,
        cancel: &CancellationToken,
    ) -> Result<(DirectCopyCapability, bool, bool, String), FileTransferError> {
        if source_id == destination_id {
            return Err(fail("same endpoint copies use relay"));
        }
        let connections = self.list_connections().await;
        let target = connections
            .iter()
            .find(|c| c.id == destination_id)
            .ok_or_else(|| fail("destination connection not found"))?;
        if target.jump.is_some() || target.proxy.is_some() {
            return Err(fail(
                "direct copy requires a directly reachable destination; choose relay",
            ));
        }
        if target.host.starts_with('-')
            || !target
                .host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b))
            || target.username.starts_with('-')
            || !target
                .username
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(fail("direct copy endpoint cannot be represented safely"));
        }
        let (owner, _) = crate::file_transfer::operation(
            cancel,
            self.inner.file_client_for_generation(source_id, sg),
        )
        .await?;
        let (destination_owner, _) = crate::file_transfer::operation(
            cancel,
            self.inner.file_client_for_generation(destination_id, dg),
        )
        .await?;
        let generation = owner
            .transport_generation()
            .ok_or_else(|| fail("source generation unavailable"))?;
        let trusted = self
            .list_host_keys(destination_id)
            .await
            .map_err(|e| fail(&e.to_string()))?;
        let output=run(self,source_id,Some(generation),&format!("command -v ssh >/dev/null && command -v ssh-keyscan >/dev/null && ssh-keyscan -T 5 -p {} -- {} 2>/dev/null",target.port,quote(&target.host)),cancel).await?;
        let key=output.lines().filter_map(|line|{
            let parts:Vec<_>=line.split_whitespace().collect();if parts.len()!=3{return None;}
            let key=PublicKey::from_openssh(&format!("{} {}",parts[1],parts[2])).ok()?;
            let fingerprint=key.fingerprint(HashAlg::Sha256).to_string();
            trusted.iter().any(|t|!t.revoked&&t.fingerprint==fingerprint).then_some((key,fingerprint))
        }).next().ok_or_else(||fail("source-side destination Host Key did not match application trust; choose relay"))?;
        let host = if target.port == 22 {
            target.host.clone()
        } else {
            format!("[{}]:{}", target.host, target.port)
        };
        let cap = DirectCopyCapability {
            host: target.host.clone(),
            port: target.port,
            user: target.username.clone(),
            known_host: format!(
                "{host} {}",
                key.0.to_openssh().map_err(|e| fail(&e.to_string()))?
            ),
            source_generation: generation,
        };
        let tools=run(self,source_id,Some(generation),"command -v scp >/dev/null && printf 'scp\\n'; command -v rsync >/dev/null && printf 'rsync\\n'; true",cancel).await?;
        let target_tools = run(
            self,
            destination_id,
            destination_owner.transport_generation(),
            "command -v rsync >/dev/null && printf 'rsync\\n'; true",
            cancel,
        )
        .await?;
        Ok((
            cap,
            tools.lines().any(|l| l == "scp"),
            tools.lines().any(|l| l == "rsync") && target_tools.lines().any(|l| l == "rsync"),
            key.1,
        ))
    }
    pub(super) async fn execute_direct_copy_file(
        &self,
        request: &ServerCopyRequest,
        cap: &DirectCopyCapability,
        source: &str,
        stage: &str,
        cancel: &CancellationToken,
    ) -> Result<(), FileTransferError> {
        let host = if cap.host.contains(':') {
            format!("[{}]", cap.host)
        } else {
            cap.host.clone()
        };
        let target = format!("{}@{host}:{stage}", cap.user);
        let command=match request.spec.method {
            ServerCopyMethod::Scp=>format!("scp -F /dev/null -s -B -oBatchMode=yes -oControlMaster=no -oControlPath=none -oStrictHostKeyChecking=yes -oUserKnownHostsFile=\"$kh\" -oGlobalKnownHostsFile=/dev/null -P {} -- {} {}",cap.port,quote(source),quote(&target)),
            ServerCopyMethod::Rsync=>format!("rsync --protect-args --checksum -e \"ssh -F /dev/null -oBatchMode=yes -oControlMaster=no -oControlPath=none -oStrictHostKeyChecking=yes -oUserKnownHostsFile=$kh -oGlobalKnownHostsFile=/dev/null -p {}\" -- {} {}",cap.port,quote(source),quote(&target)),
            ServerCopyMethod::Relay=>return Err(fail("direct copy method required")),
        };
        run(
            self,
            &request.spec.source_connection_id,
            Some(cap.source_generation),
            &with_trust(cap, &command),
            cancel,
        )
        .await?;
        Ok(())
    }
}
fn ssh_options(cap: &DirectCopyCapability) -> String {
    format!("-F /dev/null -oBatchMode=yes -oControlMaster=no -oControlPath=none -oStrictHostKeyChecking=yes -oUserKnownHostsFile=\"$kh\" -oGlobalKnownHostsFile=/dev/null -oConnectTimeout=5 -oConnectionAttempts=1 -p {} -l {}",cap.port,quote(&cap.user))
}
fn with_trust(cap: &DirectCopyCapability, command: &str) -> String {
    format!("set -eu; umask 077; kh=$(mktemp /tmp/deepagent-copy-trust.XXXXXXXX); trap 'rm -f \"$kh\"' EXIT HUP INT TERM; printf '%s\\n' {} > \"$kh\"; {command}",quote(&cap.known_host))
}
fn fail(message: &str) -> FileTransferError {
    FileTransferError::new("unsupported", message)
}
