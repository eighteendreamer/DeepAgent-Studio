//! Bounds incoming response frames before russh-sftp allocates the packet body.
use std::io;
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Local receive policy, not a limit advertised by every SFTP server.
pub(crate) const MAX_RECEIVE_PACKET_BYTES: u32 = 1024 * 1024;

/// Validates a response header before the SFTP decoder allocates its payload.
/// It leaves decoding, request IDs and protocol handling to russh-sftp.
pub(crate) struct PacketStream<S> {
    inner: S,
    max_packet: u32,
    header: [u8; 4],
    received: usize,
    sent: usize,
    remaining: usize,
    stopped: bool,
}

pub(crate) fn packet_stream<S>(inner: S, max_packet: u32) -> PacketStream<S> {
    PacketStream {
        inner,
        max_packet,
        header: [0; 4],
        received: 0,
        sent: 0,
        remaining: 0,
        stopped: false,
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for PacketStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if output.remaining() == 0 || this.stopped {
            return Poll::Ready(Ok(()));
        }
        if this.remaining > 0 {
            let count = this.remaining.min(output.remaining());
            let mut part = ReadBuf::new(output.initialize_unfilled_to(count));
            let result = ready!(Pin::new(&mut this.inner).poll_read(cx, &mut part));
            let read = part.filled().len();
            if let Err(error) = result {
                this.stopped = true;
                return Poll::Ready(Err(error));
            }
            if read == 0 {
                this.stopped = true;
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated SFTP packet",
                )));
            }
            this.remaining -= read;
            output.advance(read);
            return Poll::Ready(Ok(()));
        }
        while this.received < this.header.len() {
            let mut part = ReadBuf::new(&mut this.header[this.received..]);
            let result = ready!(Pin::new(&mut this.inner).poll_read(cx, &mut part));
            let read = part.filled().len();
            if let Err(error) = result {
                this.stopped = true;
                return Poll::Ready(Err(error));
            }
            if read == 0 {
                this.stopped = true;
                return Poll::Ready(if this.received == 0 {
                    Ok(())
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "truncated SFTP header",
                    ))
                });
            }
            this.received += read;
        }
        let length = u32::from_be_bytes(this.header);
        if length == 0 || length > this.max_packet {
            this.stopped = true;
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SFTP packet exceeds local receive bound or is empty",
            )));
        }
        let count = (this.header.len() - this.sent).min(output.remaining());
        output.put_slice(&this.header[this.sent..this.sent + count]);
        this.sent += count;
        if this.sent == this.header.len() {
            this.remaining = length as usize;
            this.received = 0;
            this.sent = 0;
        }
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PacketStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, bytes)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn oversized_frame_is_rejected_before_its_body_is_read() {
        let (mut server, client) = tokio::io::duplex(64);
        server
            .write_all(&[0, 0, 0, 5, 1, 2, 3, 4, 5])
            .await
            .unwrap();
        server.shutdown().await.unwrap();
        let mut stream = packet_stream(client, 4);
        let mut bytes = Vec::new();
        let error = stream.read_to_end(&mut bytes).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            bytes.is_empty(),
            "invalid header must not reach the SFTP decoder"
        );
        assert_eq!(
            stream.read(&mut [0]).await.unwrap(),
            0,
            "retired stream must not spin on the invalid header"
        );
    }

    #[tokio::test]
    async fn valid_fragmented_frames_and_outgoing_requests_are_preserved() {
        let (mut server, client) = tokio::io::duplex(64);
        let incoming = [0, 0, 0, 3, 7, 8, 9, 0, 0, 0, 1, 6];
        let producer = tokio::spawn(async move {
            for byte in incoming {
                server.write_all(&[byte]).await.unwrap();
                tokio::task::yield_now().await;
            }
            server.shutdown().await.unwrap();
            let mut request = Vec::new();
            server.read_to_end(&mut request).await.unwrap();
            request
        });
        let mut stream = packet_stream(client, 4);
        stream.write_all(b"request").await.unwrap();
        stream.shutdown().await.unwrap();
        let mut actual = Vec::new();
        let mut byte = [0];
        while stream.read(&mut byte).await.unwrap() != 0 {
            actual.push(byte[0]);
        }
        assert_eq!(actual, incoming);
        assert_eq!(producer.await.unwrap(), b"request");
    }

    #[tokio::test]
    async fn truncated_header_and_payload_are_errors_but_clean_eof_is_not() {
        for bytes in [vec![0, 0], vec![0, 0, 0, 3, 1]] {
            let (mut server, client) = tokio::io::duplex(64);
            server.write_all(&bytes).await.unwrap();
            server.shutdown().await.unwrap();
            let mut stream = packet_stream(client, 4);
            let error = stream.read_to_end(&mut Vec::new()).await.unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
        }
        let (mut server, client) = tokio::io::duplex(64);
        server.shutdown().await.unwrap();
        assert_eq!(
            packet_stream(client, 4)
                .read_to_end(&mut Vec::new())
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn zero_length_packet_is_rejected() {
        let (mut server, client) = tokio::io::duplex(64);
        server.write_all(&[0; 4]).await.unwrap();
        server.shutdown().await.unwrap();
        let error = packet_stream(client, 4)
            .read_to_end(&mut Vec::new())
            .await
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn raw_session_initializes_through_the_bounded_stream() {
        struct Server;
        impl russh_sftp::server::Handler for Server {
            type Error = russh_sftp::protocol::StatusCode;
            fn unimplemented(&self) -> Self::Error {
                Self::Error::OpUnsupported
            }
        }
        let (server, client) = tokio::io::duplex(128);
        let task = tokio::spawn(russh_sftp::server::run(server, Server));
        let session = russh_sftp::client::RawSftpSession::new(packet_stream(client, 1024));
        assert_eq!(session.init().await.unwrap().version, 3);
        session.close_session().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
    }
}
