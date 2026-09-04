//! The `--health-check` probe: a container `HEALTHCHECK` that re-runs this
//! same binary.
//!
//! `gcr.io/distroless/cc` ships neither a shell nor `curl`, so the only
//! executable available to probe the server is the server itself. Keeping the
//! probe here in the library rather than in `main.rs` is what makes it
//! testable: the tests below stand up a real listener on an ephemeral port and
//! drive [`check_health`] against it, which is exactly what the container does.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

/// How long the probe waits before calling the server unhealthy.
///
/// Short on purpose: a container runtime runs this on a timer of its own
/// (Podman's and Docker's default `--health-timeout` is 30s), and a probe that
/// outlived that budget would be killed mid-request and reported as a failure
/// with no diagnostic of its own.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Returns whether the server listening on `addr` answers `/healthz`
/// successfully.
///
/// Any failure — connection refused, timeout, a non-2xx status — is `false`.
/// The caller's only decision is the process exit code, so there is nothing a
/// richer error type could be used for here.
pub async fn check_health(addr: SocketAddr) -> bool {
    let url = format!("http://{}/healthz", probe_target(addr));
    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(client) => client,
        Err(err) => {
            tracing::error!(%err, "failed to build the health-check HTTP client");
            return false;
        }
    };
    match client.get(&url).send().await {
        Ok(response) => {
            let status = response.status();
            if !status.is_success() {
                tracing::error!(%url, %status, "health check got an unsuccessful status");
            }
            status.is_success()
        }
        Err(err) => {
            tracing::error!(%url, %err, "health check could not reach the server");
            false
        }
    }
}

/// The address to actually connect to, given the address the server *bound*.
///
/// `ANAMNESIS_BIND_ADDR` defaults to `0.0.0.0:8080`, and the wildcard means
/// "every interface" only to `bind`; as a destination it is not a routable
/// host. Since the probe always runs beside the server — same container, same
/// machine — loopback is the interface it should use, and the port is the only
/// part of a wildcard bind that carries information.
fn probe_target(addr: SocketAddr) -> SocketAddr {
    if !addr.ip().is_unspecified() {
        return addr;
    }
    let loopback = match addr.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
    };
    SocketAddr::new(loopback, addr.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::Router;
    use axum::routing::get;

    fn addr(raw: &str) -> SocketAddr {
        raw.parse().expect("test address parses")
    }

    #[test]
    fn unspecified_ipv4_bind_probes_loopback() {
        assert_eq!(probe_target(addr("0.0.0.0:8080")), addr("127.0.0.1:8080"));
    }

    #[test]
    fn unspecified_ipv6_bind_probes_loopback() {
        assert_eq!(probe_target(addr("[::]:8080")), addr("[::1]:8080"));
    }

    /// A concrete bind address is already the address to talk to — rewriting
    /// it to loopback would probe the wrong interface.
    #[test]
    fn concrete_bind_addr_is_probed_as_is() {
        assert_eq!(probe_target(addr("10.1.2.3:9000")), addr("10.1.2.3:9000"));
        assert_eq!(probe_target(addr("127.0.0.1:9000")), addr("127.0.0.1:9000"));
    }

    /// Serves `router` on an ephemeral loopback port until the returned
    /// handle is aborted, yielding the address it actually got.
    async fn serve_in_background(router: Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("read the bound address");
        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve");
        });
        (addr, handle)
    }

    #[tokio::test]
    async fn a_live_server_is_healthy() {
        let router = Router::new().route("/healthz", get(|| async { "ok" }));
        let (addr, handle) = serve_in_background(router).await;

        assert!(check_health(addr).await);

        handle.abort();
    }

    /// The failure the container actually cares about: nothing is listening.
    #[tokio::test]
    async fn a_dead_port_is_not_healthy() {
        // Bind, read the port, then drop the listener: the port was real a
        // moment ago, so this is a refused connection rather than a hang.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("read the bound address");
        drop(listener);

        assert!(!check_health(addr).await);
    }

    /// A server that is up but broken is not healthy either — the probe reads
    /// the status, not merely whether the connection succeeded.
    #[tokio::test]
    async fn an_unsuccessful_status_is_not_healthy() {
        let router = Router::new().route(
            "/healthz",
            get(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
        );
        let (addr, handle) = serve_in_background(router).await;

        assert!(!check_health(addr).await);

        handle.abort();
    }
}
