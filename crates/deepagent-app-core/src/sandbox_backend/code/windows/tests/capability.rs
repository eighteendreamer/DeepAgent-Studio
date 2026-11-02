//! Opt-in production SDK execution and synthetic OS containment acceptance.
use super::*;
use deepagent_runtime::workflow::code::{CodeInput, CodeOutput};
use std::net::{TcpListener, TcpStream, UdpSocket};

#[test]
#[ignore = "requires explicit managed SDK and unpackaged terminal; executes the production strict backend"]
fn h178_production_node_computes_and_denies_files_network() {
    let (_root, mut request) = sdk_request();
    request.code.script = "function main({ input }) { return { result: input * 7 }; }".into();
    request.code.inputs = vec![CodeInput {
        name: "input".into(),
        value: json!(6),
    }];
    request.code.outputs = vec![CodeOutput {
        name: "result".into(),
        value_type: "number".into(),
    }];
    let response =
        execute(request, Arc::new(AtomicBool::new(false))).expect("actual production computation");
    assert_eq!(response.outputs["result"], 42);
    assert_eq!(response.receipt.isolation, CodeIsolation::WindowsLpacV1);
    assert_eq!(response.receipt.exit_code, 0);
    println!("production_fixed_compute=42");
    for host in ["127.0.0.1", "::1"] {
        evaluate_boundaries(host);
    }
    println!("production_compute_files_ipv4_ipv6_boundaries_passed=true");
}

fn evaluate_boundaries(host: &str) {
    let (root, mut request) = sdk_request();
    let canary = root.path().join("synthetic-host-only.txt");
    let marker = b"H178_SYNTHETIC_NO_USER_DATA";
    std::fs::write(&canary, marker).unwrap();
    let outside_write = root.path().join("must-not-create.txt");
    let tcp = TcpListener::bind((host, 0)).unwrap();
    let tcp_address = tcp.local_addr().unwrap();
    let client = TcpStream::connect(tcp_address).unwrap();
    let (accepted, _) = tcp.accept().unwrap();
    drop((client, accepted));
    tcp.set_nonblocking(true).unwrap();
    let udp = UdpSocket::bind((host, 0)).unwrap();
    let udp_address = udp.local_addr().unwrap();
    let sender = UdpSocket::bind((host, 0)).unwrap();
    sender.send_to(b"positive-control", udp_address).unwrap();
    udp.set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    let mut datagram = [0u8; 32];
    assert_eq!(udp.recv_from(&mut datagram).unwrap().0, 16);
    udp.set_nonblocking(true).unwrap();
    drop(sender);
    println!("host_tcp_udp_loopback_positive_controls=true");
    request.code.script = include_str!("capability_boundaries.js").into();
    request.code.inputs = vec![CodeInput {
        name: "fixture".into(),
        value: json!({
            "canary":canary, "outsideWrite":outside_write, "host":host,
            "tcpPort":tcp_address.port(), "udpPort":udp_address.port(),
        }),
    }];
    let names = [
        "hostRead",
        "hostWrite",
        "stageWrite",
        "tcpConnect",
        "tcpListen",
        "udpSend",
        "udpBind",
    ];
    request.code.outputs = names
        .iter()
        .map(|name| CodeOutput {
            name: (*name).into(),
            value_type: "string".into(),
        })
        .collect();
    let response = execute(request, Arc::new(AtomicBool::new(false)))
        .expect("production SDK boundary fixture");
    assert_eq!(std::fs::read(canary).unwrap(), marker);
    assert!(!outside_write.exists());
    assert!(matches!(tcp.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock));
    assert!(
        matches!(udp.recv_from(&mut datagram), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    for name in names {
        let observed = response.outputs[name].as_str().unwrap();
        println!("boundary={name} host={host} observed={observed}");
        assert!(
            matches!(observed, "EACCES" | "EPERM"),
            "{name} must be denied by access policy, not timeout/refusal: {observed}"
        );
    }
    println!("synthetic_file_and_loopback_boundaries_denied=true host={host}");
}
