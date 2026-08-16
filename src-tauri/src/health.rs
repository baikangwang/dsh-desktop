//! Readiness / liveness probes for the dsh web server.
//!
//! A TCP connect is enough to know the socket is bound. During boot the server
//! answers 404 until the SPA fallback registers, so "connectable" is the correct
//! readiness signal; the browser-side SSE client then reconnects on its own.

use std::time::Duration;

/// True once a TCP connection to `(host, port)` succeeds.
pub async fn is_up(host: &str, port: u16) -> bool {
    tokio::net::TcpStream::connect((host, port)).await.is_ok()
}

/// Poll `is_up` until success or `timeout` elapses.
pub async fn wait_ready(host: &str, port: u16, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if is_up(host, port).await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}
