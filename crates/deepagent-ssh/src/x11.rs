//! X11 uses a per-transport fake cookie, checked and replaced before reaching the local server.
use crate::advanced::extension_error;
use crate::{SshError, SshResult};
use serde::{Deserialize, Serialize};
use std::{net::IpAddr, path::PathBuf, sync::Arc, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshX11Options {
    pub display: String,
    #[serde(default)]
    pub xauthority_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshX11Probe {
    pub available: bool,
    pub display: String,
    pub reason: Option<String>,
}

enum Endpoint {
    Tcp(IpAddr, u16),
    #[cfg(unix)]
    Unix(PathBuf),
}

struct Display {
    endpoint: Endpoint,
    number: u16,
    screen: u32,
}

impl SshX11Options {
    pub(crate) fn validate(&self) -> SshResult<()> {
        self.parsed()?;
        if self
            .xauthority_path
            .as_deref()
            .is_some_and(|path| path.is_empty() || path.contains('\0'))
        {
            return Err(SshError::InvalidConfig("invalid Xauthority path".into()));
        }
        Ok(())
    }

    fn parsed(&self) -> SshResult<Display> {
        let invalid = || {
            SshError::InvalidConfig(
                "X11 display must be a local display such as 127.0.0.1:0 or :0".into(),
            )
        };
        if self.display.chars().any(char::is_whitespace) {
            return Err(invalid());
        }
        let (host, suffix) = self.display.rsplit_once(':').ok_or_else(invalid)?;
        let (number, screen) = suffix.split_once('.').unwrap_or((suffix, "0"));
        let number: u16 = number.parse().map_err(|_| invalid())?;
        let screen: u32 = screen.parse().map_err(|_| invalid())?;
        let port = 6000u16.checked_add(number).ok_or_else(invalid)?;
        let endpoint = if host.is_empty() || host == "unix" {
            #[cfg(unix)]
            {
                Endpoint::Unix(PathBuf::from(format!("/tmp/.X11-unix/X{number}")))
            }
            #[cfg(not(unix))]
            {
                Endpoint::Tcp(std::net::Ipv4Addr::LOCALHOST.into(), port)
            }
        } else {
            let address: IpAddr = host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse()
                .map_err(|_| invalid())?;
            if !address.is_loopback() {
                return Err(invalid());
            }
            Endpoint::Tcp(address, port)
        };
        Ok(Display {
            endpoint,
            number,
            screen,
        })
    }
}

trait XStream: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> XStream for T {}

async fn connect(display: &Display) -> SshResult<Box<dyn XStream>> {
    let connection = async {
        match &display.endpoint {
            Endpoint::Tcp(ip, port) => Ok(Box::new(
                tokio::net::TcpStream::connect((*ip, *port)).await?,
            ) as Box<dyn XStream>),
            #[cfg(unix)]
            Endpoint::Unix(path) => {
                Ok(Box::new(tokio::net::UnixStream::connect(path).await?) as Box<dyn XStream>)
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(2), connection)
        .await
        .map_err(|_| extension_error("X11", "local X server connection timed out"))?
        .map_err(|error: std::io::Error| {
            extension_error("X11", format!("local X server unavailable: {error}"))
        })
}

fn read_cookie(bytes: &[u8], number: u16) -> SshResult<Vec<u8>> {
    let mut rest = bytes;
    let invalid = || extension_error("X11", "malformed Xauthority file");
    let mut matching = Vec::new();
    while !rest.is_empty() {
        if rest.len() < 2 {
            return Err(invalid());
        }
        let family = u16::from_be_bytes([rest[0], rest[1]]);
        rest = &rest[2..];
        let mut fields = Vec::new();
        for _ in 0..4 {
            if rest.len() < 2 {
                return Err(invalid());
            }
            let length = u16::from_be_bytes([rest[0], rest[1]]) as usize;
            rest = &rest[2..];
            if rest.len() < length {
                return Err(invalid());
            }
            fields.push(&rest[..length]);
            rest = &rest[length..];
        }
        let local_address = match family {
            256 | 65535 => true,
            0 => fields[0].len() == 4 && fields[0][0] == 127,
            6 => fields[0] == std::net::Ipv6Addr::LOCALHOST.octets(),
            _ => false,
        };
        if local_address
            && fields[1] == number.to_string().as_bytes()
            && fields[2] == b"MIT-MAGIC-COOKIE-1"
            && fields[3].len() == 16
        {
            if !matching
                .iter()
                .any(|existing: &Vec<u8>| existing.as_slice() == fields[3])
            {
                matching.push(fields[3].to_vec());
            }
        }
    }
    if matching.len() != 1 {
        return Err(extension_error(
            "X11",
            "no unique local MIT-MAGIC-COOKIE-1 identity for this display",
        ));
    }
    Ok(matching.remove(0))
}

pub(crate) struct X11Runtime {
    display: Display,
    actual_cookie: Vec<u8>,
    fake_cookie: [u8; 16],
}

impl X11Runtime {
    pub(crate) async fn prepare(options: &SshX11Options) -> SshResult<Arc<Self>> {
        options.validate()?;
        let display = options.parsed()?;
        drop(connect(&display).await?);
        let path = options
            .xauthority_path
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("XAUTHORITY").map(PathBuf::from))
            .or_else(|| dirs::home_dir().map(|home| home.join(".Xauthority")))
            .ok_or_else(|| extension_error("X11", "Xauthority file is unavailable"))?;
        let bytes = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(path)?
                .take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            Ok::<_, std::io::Error>(bytes)
        })
        .await
        .map_err(|_| extension_error("X11", "Xauthority reader failed"))?
        .map_err(|error| extension_error("X11", format!("cannot read Xauthority: {error}")))?;
        if bytes.len() > 1024 * 1024 {
            return Err(extension_error("X11", "Xauthority exceeds 1 MiB"));
        }
        let actual_cookie = read_cookie(&bytes, display.number)?;
        let fake_cookie = *uuid::Uuid::new_v4().as_bytes();
        Ok(Arc::new(Self {
            display,
            actual_cookie,
            fake_cookie,
        }))
    }

    pub(crate) fn screen(&self) -> u32 {
        self.display.screen
    }
    pub(crate) fn cookie_hex(&self) -> String {
        self.fake_cookie
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    pub(crate) async fn bridge(
        &self,
        channel: russh::Channel<russh::client::Msg>,
    ) -> SshResult<()> {
        self.bridge_stream(channel.into_stream()).await
    }

    async fn bridge_stream<R: AsyncRead + AsyncWrite + Unpin>(
        &self,
        mut remote: R,
    ) -> SshResult<()> {
        let setup = tokio::time::timeout(Duration::from_secs(5), async {
            let mut header = [0u8; 12];
            remote.read_exact(&mut header).await?;
            let little = match header[0] {
                b'l' => true,
                b'B' => false,
                _ => return Err(std::io::ErrorKind::InvalidData.into()),
            };
            let word = |a, b| {
                if little {
                    u16::from_le_bytes([a, b])
                } else {
                    u16::from_be_bytes([a, b])
                }
            };
            if word(header[2], header[3]) != 11 {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            let name = usize::from(word(header[6], header[7]));
            let data = usize::from(word(header[8], header[9]));
            if name != 18 || data != 16 {
                return Err(std::io::ErrorKind::PermissionDenied.into());
            }
            let padded_name = (name + 3) & !3;
            let mut payload = vec![0; padded_name + ((data + 3) & !3)];
            remote.read_exact(&mut payload).await?;
            let difference = payload[padded_name..padded_name + data]
                .iter()
                .zip(self.fake_cookie)
                .fold(0u8, |difference, (actual, expected)| {
                    difference | (actual ^ expected)
                });
            if &payload[..name] != b"MIT-MAGIC-COOKIE-1" || difference != 0 {
                return Err(std::io::ErrorKind::PermissionDenied.into());
            }
            payload[padded_name..padded_name + data].copy_from_slice(&self.actual_cookie);
            let mut local = connect(&self.display)
                .await
                .map_err(std::io::Error::other)?;
            local.write_all(&header).await?;
            local.write_all(&payload).await?;
            Ok::<_, std::io::Error>(local)
        })
        .await
        .map_err(|_| extension_error("X11", "X11 setup timed out"))?
        .map_err(|_| extension_error("X11", "X11 cookie validation or local setup failed"))?;
        let mut local = setup;
        tokio::io::copy_bidirectional(&mut remote, &mut local)
            .await
            .map_err(|error| extension_error("X11", format!("X11 stream: {error}")))?;
        Ok(())
    }
}

pub async fn probe_x11(options: SshX11Options) -> SshX11Probe {
    let result = X11Runtime::prepare(&options).await;
    SshX11Probe {
        available: result.is_ok(),
        display: options.display,
        reason: result.err().map(|error| error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(little: bool, cookie: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 12];
        bytes[0] = if little { b'l' } else { b'B' };
        for (offset, value) in [(2, 11u16), (6, 18), (8, 16)] {
            let word = if little {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            };
            bytes[offset..offset + 2].copy_from_slice(&word);
        }
        bytes.extend_from_slice(b"MIT-MAGIC-COOKIE-1\0\0");
        bytes.extend_from_slice(cookie);
        bytes
    }

    #[tokio::test]
    async fn setup_checks_cookie_before_connecting_and_replaces_it_in_both_byte_orders() {
        tokio::time::timeout(Duration::from_secs(5), async {
            for little in [false, true] {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let runtime = Arc::new(X11Runtime {
                    display: Display {
                        endpoint: Endpoint::Tcp(
                            "127.0.0.1".parse().unwrap(),
                            listener.local_addr().unwrap().port(),
                        ),
                        number: 0,
                        screen: 0,
                    },
                    actual_cookie: vec![9; 16],
                    fake_cookie: [7; 16],
                });
                let (mut remote, bridge) = tokio::io::duplex(1024);
                let owner = runtime.clone();
                let actor = tokio::spawn(async move { owner.bridge_stream(bridge).await });
                remote.write_all(&setup(little, &[8; 16])).await.unwrap();
                assert!(actor.await.unwrap().is_err());
                assert!(
                    tokio::time::timeout(Duration::from_millis(30), listener.accept())
                        .await
                        .is_err(),
                    "invalid cookie must never reach the local X server"
                );

                let (mut remote, bridge) = tokio::io::duplex(1024);
                let owner = runtime.clone();
                let actor = tokio::spawn(async move { owner.bridge_stream(bridge).await });
                remote.write_all(&setup(little, &[7; 16])).await.unwrap();
                let (mut local, _) = listener.accept().await.unwrap();
                let mut bytes = vec![0; 48];
                local.read_exact(&mut bytes).await.unwrap();
                assert_eq!(bytes, setup(little, &[9; 16]));
                local.write_all(b"x-response").await.unwrap();
                let mut response = [0; 10];
                remote.read_exact(&mut response).await.unwrap();
                assert_eq!(&response, b"x-response");
                drop(local);
                drop(remote);
                actor.await.unwrap().unwrap();
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn malformed_or_remote_xauthority_never_becomes_an_identity() {
        assert!(read_cookie(&[0], 0).is_err());
        let mut record = vec![0, 0];
        for field in [&[8, 8, 8, 8][..], b"0", b"MIT-MAGIC-COOKIE-1", &[7; 16]] {
            record.extend_from_slice(&(field.len() as u16).to_be_bytes());
            record.extend_from_slice(field);
        }
        assert!(read_cookie(&record, 0).is_err());
        record[4..8].copy_from_slice(&[127, 0, 0, 1]);
        assert_eq!(read_cookie(&record, 0).unwrap(), vec![7; 16]);
    }
}
