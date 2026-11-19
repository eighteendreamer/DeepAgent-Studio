//! A bounded TCP-route handshake. SSH host verification and authentication stay in russh.
//! HTTP CONNECT follows RFC 9110 §9.3.6; SOCKS CONNECT/auth follow RFC 1928/1929.

use crate::{SshConnectionConfig, SshError, SshHostIdentity, SshResult};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const PROXY_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_HTTP_HEADER: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshProxyType {
    HttpConnect,
    Socks5,
}

impl SshProxyType {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::HttpConnect => "http_connect",
            Self::Socks5 => "socks5",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshProxyEndpoint {
    pub mode: SshProxyType,
    pub host: String,
    pub port: u16,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshProxyConfig {
    pub mode: SshProxyType,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshProxySettingsRequest {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum SshProxyRequest {
    Direct,
    HttpConnect(SshProxySettingsRequest),
    Socks5(SshProxySettingsRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshProxyDto {
    pub mode: SshProxyType,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub has_password: bool,
}

impl std::fmt::Debug for SshProxyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshProxyConfig")
            .field("endpoint", &self.endpoint())
            .finish_non_exhaustive()
    }
}
impl std::fmt::Debug for SshProxyRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SshProxyRequest { .. }")
    }
}
impl std::fmt::Debug for SshProxySettingsRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SshProxySettingsRequest { .. }")
    }
}

impl SshProxyConfig {
    pub fn endpoint(&self) -> SshProxyEndpoint {
        SshProxyEndpoint {
            mode: self.mode,
            host: self.host.clone(),
            port: self.port,
        }
    }
    pub(crate) fn dto(&self) -> SshProxyDto {
        SshProxyDto {
            mode: self.mode,
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            has_password: self.password.is_some(),
        }
    }
    pub(crate) fn validate(&self) -> SshResult<()> {
        validate_host(&self.host, self.port)?;
        if self
            .username
            .as_ref()
            .is_some_and(|name| name.is_empty() || name.chars().any(char::is_control))
            || (self.username.is_none() && self.password.is_some())
        {
            return Err(invalid("proxy credentials require a nonempty username"));
        }
        if let Some(username) = &self.username {
            if self.mode == SshProxyType::HttpConnect && username.contains(':') {
                return Err(invalid("HTTP Basic proxy username cannot contain ':'"));
            }
            if self.mode == SshProxyType::Socks5
                && (!(1..=255).contains(&username.len())
                    || !(1..=255).contains(&self.password.as_deref().unwrap_or_default().len()))
            {
                return Err(invalid(
                    "SOCKS5 username and password must each contain 1 to 255 bytes",
                ));
            }
        }
        Ok(())
    }
}

pub(crate) fn resolve_proxy(
    request: Option<SshProxyRequest>,
    previous: Option<&SshProxyConfig>,
) -> SshResult<Option<SshProxyConfig>> {
    let Some(request) = request else {
        return Ok(previous.cloned());
    };
    let (mode, settings) = match request {
        SshProxyRequest::Direct => return Ok(None),
        SshProxyRequest::HttpConnect(settings) => (SshProxyType::HttpConnect, settings),
        SshProxyRequest::Socks5(settings) => (SshProxyType::Socks5, settings),
    };
    let host = validate_host(&settings.host, settings.port)?;
    let username = settings.username.filter(|name| !name.is_empty());
    let same = previous.filter(|old| {
        old.mode == mode
            && old.host == host
            && old.port == settings.port
            && old.username == username
    });
    let password = settings
        .password
        .or_else(|| same.and_then(|old| old.password.clone()))
        .filter(|secret| !secret.is_empty());
    let proxy = SshProxyConfig {
        mode,
        host,
        port: settings.port,
        username,
        password,
    };
    proxy.validate()?;
    Ok(Some(proxy))
}

fn validate_host(host: &str, port: u16) -> SshResult<String> {
    let identity = SshHostIdentity::direct(host, port)?;
    if identity.host.parse::<IpAddr>().is_err()
        && (!identity.host.is_ascii()
            || identity
                .host
                .bytes()
                .any(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))))
    {
        return Err(invalid(
            "proxy route requires an IP address or ASCII host name",
        ));
    }
    Ok(identity.host)
}

pub(crate) async fn open_tcp(config: &SshConnectionConfig) -> SshResult<TcpStream> {
    let Some(proxy) = &config.proxy else {
        return TcpStream::connect((config.host.as_str(), config.port))
            .await
            .map_err(|error| {
                SshError::ConnectionLost(format!("SSH TCP connection: {:?}", error.kind()))
            });
    };
    proxy.validate()?;
    let target = validate_host(&config.host, config.port)?;
    tokio::time::timeout(PROXY_TIMEOUT, async {
        let mut stream = TcpStream::connect((proxy.host.as_str(), proxy.port))
            .await
            .map_err(|error| {
                failure(
                    "connect",
                    format!("TCP connection failed ({:?})", error.kind()),
                )
            })?;
        match proxy.mode {
            SshProxyType::HttpConnect => {
                http_connect(&mut stream, proxy, &target, config.port).await?
            }
            SshProxyType::Socks5 => socks_connect(&mut stream, proxy, &target, config.port).await?,
        }
        Ok(stream)
    })
    .await
    .map_err(|_| failure("connect", "TCP/protocol handshake exceeded 10 seconds"))?
}

async fn http_connect(
    stream: &mut TcpStream,
    proxy: &SshProxyConfig,
    host: &str,
    port: u16,
) -> SshResult<()> {
    let authority = if matches!(host.parse::<IpAddr>(), Ok(IpAddr::V6(_))) {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(username) = &proxy.username {
        let encoded = base64::engine::general_purpose::STANDARD.encode(format!(
            "{username}:{}",
            proxy.password.as_deref().unwrap_or_default()
        ));
        request.push_str(&format!("Proxy-Authorization: Basic {encoded}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(protocol_io)?;
    for _ in 0..5 {
        let mut header = Vec::new();
        // Consume exactly the header: a buffered reader could swallow the SSH banner.
        while !header.ends_with(b"\r\n\r\n") {
            if header.len() == MAX_HTTP_HEADER {
                return Err(failure("protocol", "HTTP response headers exceed 16 KiB"));
            }
            header.push(stream.read_u8().await.map_err(protocol_io)?);
        }
        let status = http_status(&header)?;
        match status {
            200..=299 => return Ok(()), // RFC 9110: ignore Content-Length/Transfer-Encoding here.
            100..=199 if status != 101 => continue,
            407 => {
                return Err(failure(
                    "authentication",
                    "HTTP CONNECT credentials were rejected (407)",
                ))
            }
            other => {
                return Err(failure(
                    "connect",
                    format!("HTTP CONNECT rejected with status {other}"),
                ))
            }
        }
    }
    Err(failure(
        "protocol",
        "HTTP informational response limit exceeded",
    ))
}

fn http_status(header: &[u8]) -> SshResult<u16> {
    let invalid = || failure("protocol", "malformed HTTP CONNECT response");
    let mut headers = [httparse::EMPTY_HEADER; 128];
    let mut response = httparse::Response::new(&mut headers);
    match response.parse(header).map_err(|_| invalid())? {
        httparse::Status::Complete(length) if length == header.len() => {
            response.code.ok_or_else(invalid)
        }
        _ => Err(invalid()),
    }
}

async fn socks_connect(
    stream: &mut TcpStream,
    proxy: &SshProxyConfig,
    host: &str,
    port: u16,
) -> SshResult<()> {
    let method = if proxy.username.is_some() { 2 } else { 0 };
    stream
        .write_all(&[5, 1, method])
        .await
        .map_err(protocol_io)?;
    let mut selected = [0; 2];
    stream
        .read_exact(&mut selected)
        .await
        .map_err(protocol_io)?;
    if selected == [5, 255] {
        return Err(failure(
            "authentication",
            "SOCKS5 rejected the offered authentication method",
        ));
    }
    if selected != [5, method] {
        return Err(failure(
            "protocol",
            "SOCKS5 selected an unoffered authentication method or invalid version",
        ));
    }
    if let Some(username) = &proxy.username {
        let password = proxy.password.as_deref().unwrap_or_default();
        let mut request = vec![1, username.len() as u8];
        request.extend_from_slice(username.as_bytes());
        request.push(password.len() as u8);
        request.extend_from_slice(password.as_bytes());
        stream.write_all(&request).await.map_err(protocol_io)?;
        stream
            .read_exact(&mut selected)
            .await
            .map_err(protocol_io)?;
        if selected[0] != 1 {
            return Err(failure("protocol", "invalid SOCKS5 authentication version"));
        }
        if selected[1] != 0 {
            return Err(failure(
                "authentication",
                "SOCKS5 credentials were rejected",
            ));
        }
    }
    let mut request = vec![5, 1, 0];
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            request.push(1);
            request.extend(ip.octets());
        }
        Ok(IpAddr::V6(ip)) => {
            request.push(4);
            request.extend(ip.octets());
        }
        Err(_) => {
            request.extend([3, host.len() as u8]);
            request.extend_from_slice(host.as_bytes());
        }
    }
    request.extend(port.to_be_bytes());
    stream.write_all(&request).await.map_err(protocol_io)?;
    let mut reply = [0; 4];
    stream.read_exact(&mut reply).await.map_err(protocol_io)?;
    if reply[0] != 5 || reply[2] != 0 {
        return Err(failure(
            "protocol",
            "invalid SOCKS5 CONNECT version or reserved byte",
        ));
    }
    if reply[1] != 0 {
        return Err(failure(
            "connect",
            format!("SOCKS5 CONNECT rejected with reply {}", reply[1]),
        ));
    }
    let length = match reply[3] {
        1 => 4,
        4 => 16,
        3 => usize::from(stream.read_u8().await.map_err(protocol_io)?),
        _ => return Err(failure("protocol", "invalid SOCKS5 bound address type")),
    };
    if length == 0 {
        return Err(failure("protocol", "empty SOCKS5 bound address"));
    }
    let mut bound = vec![0; length + 2];
    stream.read_exact(&mut bound).await.map_err(protocol_io)?;
    Ok(())
}

fn invalid(reason: &str) -> SshError {
    SshError::InvalidConfig(reason.into())
}
fn failure(stage: &'static str, reason: impl Into<String>) -> SshError {
    SshError::Proxy {
        stage,
        reason: reason.into(),
    }
}
fn protocol_io(error: std::io::Error) -> SshError {
    failure(
        "protocol",
        format!("proxy stream IO failed ({:?})", error.kind()),
    )
}
