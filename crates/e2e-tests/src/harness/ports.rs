//! Ephemeral loopback ports.

use std::net::TcpListener;

/// Reserve an ephemeral port by binding to `127.0.0.1:0` and immediately
/// releasing it. A small race window remains, but processes bind within
/// milliseconds and each test owns a disjoint set of services.
pub fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
    listener
        .local_addr()
        .expect("read the ephemeral port")
        .port()
}
