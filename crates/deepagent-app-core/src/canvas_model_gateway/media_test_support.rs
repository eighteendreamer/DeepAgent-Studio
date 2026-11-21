//! Shared synthetic HTTP fixture for media response tests; never a production transport.
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

#[derive(Clone, Copy)]
pub(super) enum Framing {
    Length(usize),
    Chunked,
    Close,
}

pub(super) struct MediaWire {
    pub(super) address: SocketAddr,
    pub(super) sent: Arc<Notify>,
    pub(super) closed: Arc<Notify>,
    pub(super) requests: Arc<AtomicUsize>,
    pub(super) release: Arc<Notify>,
    pub(super) task: tokio::task::JoinHandle<()>,
}

impl MediaWire {
    pub(super) async fn start(
        body: Vec<u8>,
        framing: Framing,
        json_body: bool,
        stall: bool,
    ) -> Self {
        Self::media(
            body,
            framing,
            if json_body {
                "application/json"
            } else {
                "image/png"
            },
            stall,
        )
        .await
    }

    pub(super) async fn media(
        body: Vec<u8>,
        framing: Framing,
        mime: &'static str,
        stall: bool,
    ) -> Self {
        Self::media_status(body, framing, mime, stall, 200).await
    }

    pub(super) async fn media_status(
        body: Vec<u8>,
        framing: Framing,
        mime: &'static str,
        stall: bool,
        status: u16,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let sent = Arc::new(Notify::new());
        let closed = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let released = release.clone();
        let requests = Arc::new(AtomicUsize::new(0));
        let (ready, disconnected, count) = (sent.clone(), closed.clone(), requests.clone());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            loop {
                let n = socket.read(&mut buffer).await.unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buffer[..n]);
                assert!(request.len() <= 65536);
                if let Some(end) = request.windows(4).position(|s| s == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&request[..end]);
                    let length = header
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            count.fetch_add(1, Ordering::SeqCst);
            let headers = match framing {
                Framing::Length(n) => format!("Content-Length: {n}\r\n"),
                Framing::Chunked => "Transfer-Encoding: chunked\r\n".into(),
                Framing::Close => String::new(),
            };
            let _ = socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Type: {mime}\r\n{headers}Connection: close\r\n\r\n").as_bytes()).await;
            let write = async {
                for chunk in body.chunks(16 * 1024) {
                    if matches!(framing, Framing::Chunked) {
                        socket
                            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                            .await?;
                    }
                    socket.write_all(chunk).await?;
                    if matches!(framing, Framing::Chunked) {
                        socket.write_all(b"\r\n").await?;
                    }
                }
                ready.notify_one();
                if stall {
                    tokio::select! {
                        _ = released.notified() => {},
                        _ = socket.read(&mut buffer) => return Ok(()),
                    }
                }
                if matches!(framing, Framing::Chunked) {
                    socket.write_all(b"0\r\n\r\n").await?;
                }
                Ok::<(), std::io::Error>(())
            }
            .await;
            let _ = write; // Early peer refusal may end a fixture write.
            disconnected.notify_one();
        });
        Self {
            address,
            sent,
            closed,
            release,
            requests,
            task,
        }
    }

    pub(super) fn url(&self) -> String {
        format!(
            "http://public.fixture.invalid:{}/image.png?signature=synthetic-h167",
            self.address.port()
        )
    }

    /// Measure refusal after the fixture has delivered its bytes, while it still
    /// withholds EOF. The request's own deadline bounds connection/body delivery;
    /// neither local transfer throughput nor fixture preparation is an EOF wait.
    /// An early refusal (for example Content-Length) need not wait for `sent`.
    pub(super) async fn before_eof<F: Future>(
        &self,
        work: F,
    ) -> Result<F::Output, tokio::time::error::Elapsed> {
        let started = std::time::Instant::now();
        tokio::pin!(work);
        tokio::select! {
            result = &mut work => {
                eprintln!("media refusal before delivery finished: {:?}", started.elapsed());
                Ok(result)
            },
            _ = self.sent.notified() => {
                let delivered = started.elapsed();
                let result = tokio::time::timeout(std::time::Duration::from_secs(2), work).await;
                eprintln!("media delivery: {delivered:?}; refusal after delivery: {:?}", started.elapsed().saturating_sub(delivered));
                result
            },
        }
    }

    pub(super) async fn close(&mut self) {
        self.task.abort();
        let result = (&mut self.task).await;
        assert!(result.is_ok() || result.unwrap_err().is_cancelled());
    }
}

impl Drop for MediaWire {
    fn drop(&mut self) {
        self.task.abort();
    }
}
