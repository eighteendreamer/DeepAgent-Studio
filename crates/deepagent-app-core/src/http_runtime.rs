//! Public HTTP broker for typed tools. This is a local host policy, not a
//! provider API or an OS sandbox: isolated backends without a broker fail closed.
//!
//! Reference: deepseek-harness/web-fetch-http (validate all addresses, pin each
//! hop, same-origin redirects). IP exclusions follow IANA special-use registries;
//! IPv6 transition/translation ranges are conservatively unavailable.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use deepagent_builtins::http_request::{HttpRequest, HttpRequestClient, MAX_RESPONSE_BYTES};
use deepagent_core::error::{CoreError, Result};
use deepagent_tools::ToolExecutionContext;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, LOCATION};
use reqwest::Url;
use serde_json::Value;

const MAX_REDIRECTS: usize = 5;

#[async_trait]
trait AddressResolver: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>>;
}

struct SystemResolver;

#[async_trait]
impl AddressResolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        tokio::net::lookup_host((host, port))
            .await
            .map(|addresses| addresses.collect())
            .map_err(|error| CoreError::other(format!("HttpDnsFailed: {error}")))
    }
}

pub(crate) struct PublicHttpClient {
    resolver: Arc<dyn AddressResolver>,
    // Only unit fixtures can map validated public addresses to a local listener.
    // This route substitution does not exist in a production build.
    #[cfg(test)]
    fixture_route: Option<SocketAddr>,
}

impl Default for PublicHttpClient {
    fn default() -> Self {
        Self {
            resolver: Arc::new(SystemResolver),
            #[cfg(test)]
            fixture_route: None,
        }
    }
}

fn denied(message: &str) -> CoreError {
    CoreError::invalid(format!("NetworkDenied: {message}"))
}

fn public_ipv4(ip: Ipv4Addr) -> bool {
    let value = u32::from(ip);
    // Deliberately exclude entire special-purpose ranges, including globally
    // reachable exceptions within them. Availability must not widen this policy.
    const EXCLUDED: &[(u32, u32)] = &[
        (0x00000000, 8),
        (0x0a000000, 8),
        (0x64400000, 10),
        (0x7f000000, 8),
        (0xa9fe0000, 16),
        (0xac100000, 12),
        (0xc0000000, 24),
        (0xc0000200, 24),
        (0xc0586300, 24),
        (0xc0a80000, 16),
        (0xc6120000, 15),
        (0xc6336400, 24),
        (0xcb007100, 24),
        (0xe0000000, 4),
        (0xf0000000, 4),
    ];
    !EXCLUDED
        .iter()
        .any(|(prefix, bits)| value >> (32 - bits) == prefix >> (32 - bits))
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_ipv4(ip),
        IpAddr::V6(ip) => {
            let s = ip.segments();
            // Only current global-unicast allocation; no mapped IPv4, ULA,
            // link-local, multicast, NAT64 well-known/local or zone identifiers.
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && s[1] < 0x0200)
                && !(s[0] == 0x2001 && s[1] == 0x0db8)
                && s[0] != 0x2002
                && !(s[0] == 0x3fff && (s[1] & 0xf000) == 0)
        }
    }
}

fn parse_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).map_err(|_| denied("invalid HTTP URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(denied(
            "only explicit HTTP(S) URLs without userinfo or fragment are allowed",
        ));
    }
    let host = url.host_str().unwrap().trim_matches(['[', ']']);
    if let Ok(ip) = host.parse::<IpAddr>() {
        if !public_ip(ip) {
            return Err(denied("non-public IP destination"));
        }
    }
    Ok(url)
}

fn request_headers(request: &HttpRequest) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for (key, value) in &request.headers {
        let name = HeaderName::from_bytes(key.as_bytes())
            .map_err(|_| denied("invalid HTTP header name"))?;
        // The broker owns routing/framing. These must not override the pinned
        // destination, hop policy, or generate a second request on a connection.
        if matches!(
            name.as_str(),
            "host"
                | "connection"
                | "proxy-authorization"
                | "proxy-connection"
                | "transfer-encoding"
                | "content-length"
                | "upgrade"
                | "trailer"
                | "te"
        ) {
            return Err(denied(
                "routing, proxy and framing headers are broker-owned",
            ));
        }
        headers.insert(
            name,
            HeaderValue::from_str(value).map_err(|_| denied("invalid HTTP header value"))?,
        );
    }
    Ok(headers)
}

/// Extract an RFC 6052 embedded IPv4 address for a discovered RFC 7050 prefix.
fn embedded_ipv4(ip: Ipv6Addr, prefix_bytes: usize) -> Option<Ipv4Addr> {
    let bytes = ip.octets();
    let octets = if prefix_bytes == 12 {
        bytes[12..16].try_into().ok()?
    } else {
        if bytes[8] != 0 {
            return None;
        }
        let before = 8 - prefix_bytes;
        let mut v4 = [0; 4];
        v4[..before].copy_from_slice(&bytes[prefix_bytes..8]);
        v4[before..].copy_from_slice(&bytes[9..9 + 4 - before]);
        v4
    };
    Some(Ipv4Addr::from(octets))
}

impl PublicHttpClient {
    async fn addresses(&self, url: &Url) -> Result<Vec<SocketAddr>> {
        let host = url.host_str().unwrap().trim_matches(['[', ']']);
        let port = url
            .port_or_known_default()
            .ok_or_else(|| denied("missing HTTP port"))?;
        let addresses = match host.parse::<IpAddr>() {
            Ok(ip) => vec![SocketAddr::new(ip, port)],
            Err(_) => self.resolver.resolve(host, port).await?,
        };
        if addresses.is_empty()
            || addresses.len() > 64
            || addresses
                .iter()
                .any(|a| !public_ip(a.ip()) || a.port() != port)
        {
            return Err(denied("DNS answer must contain only public addresses"));
        }
        if addresses.iter().any(SocketAddr::is_ipv6) {
            // A network-specific DNS64 prefix can be global-unicast. Discover
            // active prefixes using the same resolver and reject the embedded
            // non-public IPv4 destination; resolution errors are fail-closed.
            let discovery = self.resolver.resolve("ipv4only.arpa", port).await?;
            for source in discovery {
                let IpAddr::V6(source) = source.ip() else {
                    continue;
                };
                for prefix in [4, 5, 6, 7, 8, 12] {
                    if !matches!(embedded_ipv4(source, prefix), Some(ip) if ip == Ipv4Addr::new(192,0,0,170) || ip == Ipv4Addr::new(192,0,0,171))
                    {
                        continue;
                    }
                    for destination in &addresses {
                        if let IpAddr::V6(ip) = destination.ip() {
                            if ip.octets()[..prefix] == source.octets()[..prefix]
                                && !matches!(embedded_ipv4(ip, prefix), Some(v4) if public_ipv4(v4))
                            {
                                return Err(denied(
                                    "DNS64 destination embeds a non-public IPv4 address",
                                ));
                            }
                        }
                    }
                }
            }
        }
        Ok(addresses)
    }

    /// Open one response after applying the broker's complete destination,
    /// redirect and retry policy.  Callers own body consumption so media can
    /// stream into artifact staging without creating a second network policy.
    pub(crate) async fn open_response(&self, request: &HttpRequest) -> Result<reqwest::Response> {
        let mut url = parse_url(&request.url)?;
        let headers = request_headers(request)?;
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| denied("invalid method"))?;
        let mut redirects = 0;
        let mut retries = 0;
        loop {
            let addresses = self.addresses(&url).await?;
            #[cfg(test)]
            let addresses = self
                .fixture_route
                .map_or(addresses.clone(), |route| vec![route]);
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .referer(false)
                .resolve_to_addrs(url.host_str().unwrap(), &addresses)
                .build()
                .map_err(|error| {
                    CoreError::other(format!("HttpClientFailed: {}", error.without_url()))
                })?;
            let response = client
                .request(method.clone(), url.clone())
                .headers(headers.clone())
                .body(request.body.clone())
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(error) if error.is_connect() && retries < request.retry_count => {
                    // No application request has been dispatched. Never retry
                    // timeouts, body errors, status errors or unknown outcomes.
                    retries += 1;
                    continue;
                }
                Err(error) => {
                    return Err(CoreError::other(format!(
                        "HttpOutcomeUnknown: {}; request was not replayed",
                        error.without_url()
                    )))
                }
            };
            if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                if redirects >= MAX_REDIRECTS || !matches!(request.method.as_str(), "GET" | "HEAD")
                {
                    return Err(denied("redirect limit or non-read request redirect"));
                }
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| denied("redirect has no valid Location"))?;
                let next = url
                    .join(location)
                    .map_err(|_| denied("invalid redirect URL"))?;
                let next = parse_url(next.as_str())?;
                if next.origin() != url.origin() {
                    return Err(denied("cross-origin redirect"));
                }
                url = next;
                redirects += 1;
                continue;
            }
            return Ok(response);
        }
    }

    async fn execute(&self, request: &HttpRequest) -> Result<Value> {
        let response = self.open_response(request).await?;
        if response
            .content_length()
            .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
        {
            return Err(CoreError::invalid(
                "HttpResponseTooLarge: declared body exceeds limit",
            ));
        }
        let status = response.status().as_u16();
        let headers: serde_json::Map<String, Value> = response
            .headers()
            .iter()
            .map(|(key, value)| {
                (
                    key.to_string(),
                    Value::String(value.to_str().unwrap_or("").to_string()),
                )
            })
            .collect();
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            CoreError::other(format!(
                "HttpOutcomeUnknown: body read failed: {}; request was not replayed",
                error.without_url()
            ))
        })? {
            if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
                return Err(CoreError::invalid(
                    "HttpResponseTooLarge: streamed body exceeds limit",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let body = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
        return Ok(serde_json::json!({"body": body, "status_code": status, "headers": headers}));
    }
}

#[async_trait]
impl HttpRequestClient for PublicHttpClient {
    fn validate(&self, request: &HttpRequest) -> Result<()> {
        request.validate()?;
        parse_url(&request.url)?;
        request_headers(request)?;
        Ok(())
    }

    async fn request(&self, request: HttpRequest, context: ToolExecutionContext) -> Result<Value> {
        self.validate(&request)?;
        let timeout = Duration::from_secs(request.timeout_secs).min(context.timeout());
        tokio::select! {
            biased;
            _ = context.cancelled() => Err(CoreError::other("HttpCancelled: request stopped; no automatic replay")),
            result = tokio::time::timeout(timeout, self.execute(&request)) => {
                result.map_err(|_| CoreError::other("HttpDeadlineExceeded: outcome may be unknown; no automatic replay"))?
            }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use tests::public_fixture_client;
