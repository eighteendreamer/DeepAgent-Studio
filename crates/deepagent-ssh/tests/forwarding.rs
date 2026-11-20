mod support;
use deepagent_ssh::{SshForwardKind, SshForwardRequest, SshForwardStatus, SshService};
use std::{sync::atomic::Ordering, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

async fn echo() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            tokio::select! {socket=listener.accept()=>{
                let (mut socket,_)=socket.unwrap();tasks.spawn(async move {let (mut read,mut write)=socket.split();let _=tokio::io::copy(&mut read,&mut write).await;});
            },_=tasks.join_next(),if !tasks.is_empty()=>{}}
        }
    });
    (address, task)
}
fn request(kind: SshForwardKind, target: std::net::SocketAddr) -> SshForwardRequest {
    SshForwardRequest {
        kind,
        bind_host: "127.0.0.1".into(),
        bind_port: 0,
        target_host: (kind != SshForwardKind::Dynamic).then(|| target.ip().to_string()),
        target_port: (kind != SshForwardKind::Dynamic).then_some(target.port()),
        allow_non_loopback: false,
    }
}
async fn exchange(mut stream: TcpStream) {
    stream.write_all(b"hello-forward").await.unwrap();
    let mut bytes = [0; 13];
    stream.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"hello-forward");
    stream.shutdown().await.unwrap();
    let mut tail = Vec::new();
    stream.read_to_end(&mut tail).await.unwrap();
}

#[tokio::test]
async fn local_forward_carries_bytes_stops_and_keeps_exec_alive() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let server = support::SshServer::start("127.0.0.1").await;
        let (target, echo) = echo().await;
        *server.counts.forward_target.write().unwrap() = Some(target);
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().into());
        let connection = server.create(&service, Some(&trust)).await;
        let handle = service.connect(&connection.id).await.unwrap();
        let generation = service
            .connection_details(&connection.id)
            .await
            .unwrap()
            .transport_generation;
        let task = service
            .start_forward(
                &connection.id,
                generation,
                request(SshForwardKind::Local, target),
            )
            .await
            .unwrap();
        assert!(task.bind_port > 0);
        exchange(
            TcpStream::connect(("127.0.0.1", task.bind_port))
                .await
                .unwrap(),
        )
        .await;
        let stopped = service
            .stop_forward(&connection.id, generation, &task.id)
            .await
            .unwrap();
        assert_eq!(stopped.status, SshForwardStatus::Stopped);
        assert!(TcpStream::connect(("127.0.0.1", task.bind_port))
            .await
            .is_err());
        assert_eq!(
            service.exec(&handle, "still-live").await.unwrap().stdout,
            "ok\n"
        );
        service.disconnect(&connection.id).await.unwrap();
        echo.abort();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn socks_domains_are_resolved_at_remote_endpoint_and_udp_is_rejected() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let server = support::SshServer::start("127.0.0.1").await;
        let (target, echo) = echo().await;
        *server.counts.forward_target.write().unwrap() = Some(target);
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().into());
        let connection = server.create(&service, Some(&trust)).await;
        service.connect(&connection.id).await.unwrap();
        let generation = service
            .connection_details(&connection.id)
            .await
            .unwrap()
            .transport_generation;
        let task = service
            .start_forward(
                &connection.id,
                generation,
                request(SshForwardKind::Dynamic, target),
            )
            .await
            .unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", task.bind_port))
            .await
            .unwrap();
        stream.write_all(&[5, 1, 0]).await.unwrap();
        let mut method = [0; 2];
        stream.read_exact(&mut method).await.unwrap();
        assert_eq!(method, [5, 0]);
        let host = b"only.remote.invalid";
        let mut connect = vec![5, 1, 0, 3, host.len() as u8];
        connect.extend_from_slice(host);
        connect.extend_from_slice(&target.port().to_be_bytes());
        stream.write_all(&connect).await.unwrap();
        let mut reply = [0; 10];
        stream.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[1], 0);
        exchange(stream).await;
        assert!(
            server
                .counts
                .forward_requests
                .read()
                .unwrap()
                .iter()
                .any(|(name, port)| name == "only.remote.invalid"
                    && *port == u32::from(target.port()))
        );
        let mut udp = TcpStream::connect(("127.0.0.1", task.bind_port))
            .await
            .unwrap();
        udp.write_all(&[5, 1, 0]).await.unwrap();
        udp.read_exact(&mut method).await.unwrap();
        udp.write_all(&[5, 3, 0, 1]).await.unwrap();
        udp.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[1], 7);
        service.disconnect(&connection.id).await.unwrap();
        assert!(TcpStream::connect(("127.0.0.1", task.bind_port))
            .await
            .is_err());
        echo.abort();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn remote_forward_reports_allocated_port_cancels_and_rejects_unregistered_channels() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let server = support::SshServer::start("127.0.0.1").await;
        let (target, echo) = echo().await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().into());
        let connection = server.create(&service, Some(&trust)).await;
        service.connect(&connection.id).await.unwrap();
        let generation = service
            .connection_details(&connection.id)
            .await
            .unwrap()
            .transport_generation;
        let task = service
            .start_forward(
                &connection.id,
                generation,
                request(SshForwardKind::Remote, target),
            )
            .await
            .unwrap();
        assert!(task.bind_port > 0);
        exchange(
            TcpStream::connect(("127.0.0.1", task.bind_port))
                .await
                .unwrap(),
        )
        .await;
        let handle = server.counts.server_handles.read().unwrap()[0].clone();
        assert!(handle
            .channel_open_forwarded_tcpip("127.0.0.1", 1, "127.0.0.1", 0)
            .await
            .is_err());
        service
            .stop_forward(&connection.id, generation, &task.id)
            .await
            .unwrap();
        assert_eq!(server.counts.remote_cancels.load(Ordering::SeqCst), 1);
        server
            .counts
            .reject_remote_forward
            .store(true, Ordering::SeqCst);
        assert!(service
            .start_forward(
                &connection.id,
                generation,
                request(SshForwardKind::Remote, target)
            )
            .await
            .is_err());
        service.disconnect(&connection.id).await.unwrap();
        echo.abort();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn generation_changes_never_restart_old_listeners_or_stop_new_jobs() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let server = support::SshServer::start("127.0.0.1").await;
        let (target, echo) = echo().await;
        *server.counts.forward_target.write().unwrap() = Some(target);
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().into());
        let connection = server.create(&service, Some(&trust)).await;
        service.connect(&connection.id).await.unwrap();
        let generation = service
            .connection_details(&connection.id)
            .await
            .unwrap()
            .transport_generation;
        let task = service
            .start_forward(
                &connection.id,
                generation,
                request(SshForwardKind::Local, target),
            )
            .await
            .unwrap();
        service.disconnect(&connection.id).await.unwrap();
        service.connect(&connection.id).await.unwrap();
        let current = service.connection_details(&connection.id).await.unwrap();
        assert_ne!(generation, current.transport_generation);
        assert!(service
            .list_forwards(&connection.id, current.transport_generation)
            .await
            .unwrap()
            .is_empty());
        assert!(service
            .start_forward(
                &connection.id,
                generation,
                request(SshForwardKind::Local, target)
            )
            .await
            .is_err());
        assert!(service
            .stop_forward(&connection.id, generation, &task.id)
            .await
            .is_err());
        assert!(TcpStream::connect(("127.0.0.1", task.bind_port))
            .await
            .is_err());
        service.disconnect(&connection.id).await.unwrap();
        echo.abort();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn stopping_a_rejected_duplicate_remote_job_preserves_the_existing_listener() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let server = support::SshServer::start("127.0.0.1").await;
        let (target, echo) = echo().await;
        let data = tempfile::tempdir().unwrap();
        let trust = data.path().join("known_hosts");
        server.write_trust(&trust);
        let service = SshService::new(data.path().into());
        let connection = server.create(&service, Some(&trust)).await;
        service.connect(&connection.id).await.unwrap();
        let generation = service
            .connection_details(&connection.id)
            .await
            .unwrap()
            .transport_generation;
        let first = service
            .start_forward(
                &connection.id,
                generation,
                request(SshForwardKind::Remote, target),
            )
            .await
            .unwrap();
        let mut duplicate = request(SshForwardKind::Remote, target);
        duplicate.bind_port = first.bind_port;
        assert!(service
            .start_forward(&connection.id, generation, duplicate)
            .await
            .is_err());
        let failed = service
            .list_forwards(&connection.id, generation)
            .await
            .unwrap()
            .into_iter()
            .find(|job| job.id != first.id)
            .unwrap();
        service
            .stop_forward(&connection.id, generation, &failed.id)
            .await
            .unwrap();
        assert_eq!(server.counts.remote_cancels.load(Ordering::SeqCst), 0);
        exchange(
            TcpStream::connect(("127.0.0.1", first.bind_port))
                .await
                .unwrap(),
        )
        .await;
        service
            .stop_forward(&connection.id, generation, &first.id)
            .await
            .unwrap();
        assert_eq!(server.counts.remote_cancels.load(Ordering::SeqCst), 1);
        service.disconnect(&connection.id).await.unwrap();
        echo.abort();
    })
    .await
    .unwrap();
}
