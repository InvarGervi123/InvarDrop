//! Connection-stage diagnostics. Never log certificates, pairing codes or keys.
use super::*;

pub(super) fn listen_port(value: Option<&str>) -> Result<u16> {
    match value {
        None => Ok(0),
        Some(value) if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => {
            value.parse().map_err(|_| Error::ListenPort)
        }
        _ => Err(Error::ListenPort),
    }
}

#[derive(Default)]
struct Failures {
    permission: bool,
    timeout: bool,
    refused: bool,
}
impl Failures {
    fn record(&mut self, kind: std::io::ErrorKind) {
        match kind {
            std::io::ErrorKind::PermissionDenied => self.permission = true,
            std::io::ErrorKind::TimedOut => self.timeout = true,
            std::io::ErrorKind::ConnectionRefused => self.refused = true,
            _ => {}
        }
    }
    fn error(&self) -> Error {
        if self.permission {
            Error::TcpPermission
        } else if self.timeout {
            Error::TcpTimeout
        } else if self.refused {
            Error::TcpRefused
        } else {
            Error::TcpConnect
        }
    }
}
pub(super) async fn tcp(addresses: Vec<SocketAddr>) -> Result<(TcpStream, SocketAddr)> {
    if addresses.is_empty() {
        return Err(Error::NoEndpoint);
    }
    let mut failures = Failures::default();
    // Preserve the existing endpoint order, attempt bound and time budgets.
    for address in addresses.into_iter().take(8) {
        #[cfg(debug_assertions)]
        eprintln!("InvarDrop TCP attempt: {address}");
        let kind = match timeout(Duration::from_millis(500), TcpStream::connect(address)).await {
            Ok(Ok(socket)) => {
                #[cfg(debug_assertions)]
                eprintln!(
                    "InvarDrop TCP connected: {:?} -> {address}",
                    socket.local_addr().ok()
                );
                return Ok((socket, address));
            }
            Ok(Err(error)) => error.kind(),
            Err(_) => std::io::ErrorKind::TimedOut,
        };
        #[cfg(debug_assertions)]
        eprintln!("InvarDrop TCP failed: {address} ({kind:?})");
        failures.record(kind);
    }
    Err(failures.error())
}
pub(super) fn tls_error(error: std::io::Error) -> Error {
    // Preserve identity/pin/signature failure semantics. I/O termination before
    // verification is a TLS-stage failure, not proof of an invalid peer key.
    match error
        .get_ref()
        .and_then(|e| e.downcast_ref::<rustls::Error>())
    {
        Some(
            rustls::Error::InvalidCertificate(_)
            | rustls::Error::NoCertificatesPresented
            | rustls::Error::General(_),
        ) => Error::Identity,
        _ => Error::TlsHandshake,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_listener_port_is_validated_without_changing_the_ephemeral_default() {
        assert_eq!(listen_port(None).unwrap(), 0);
        assert_eq!(listen_port(Some("54383")).unwrap(), 54383);
        for invalid in ["", "65536", "-1", "port", " 54383"] {
            assert!(matches!(listen_port(Some(invalid)), Err(Error::ListenPort)));
        }
    }
    #[test]
    fn occupied_configured_port_never_silently_falls_back_to_another_port() {
        let listener = listener_socket(0).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(matches!(listener_socket(port), Err(Error::ListenBind)));
    }
    #[test]
    fn occupied_ipv4_port_blocks_dual_stack_and_ipv4_fallback() {
        let listener = listener_socket_for(socket2::Domain::IPV4, 0).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(listener_socket_for(socket2::Domain::IPV4, port).is_err());
        assert!(matches!(listener_socket(port), Err(Error::ListenBind)));
        assert_eq!(listener.local_addr().unwrap().port(), port);
    }
    #[cfg(windows)]
    #[test]
    fn exclusive_listener_rejects_reuseaddr_port_hijacking() {
        use socket2::{Domain, Protocol, Socket, Type};
        // Exercise the explicit IPv4 path and the normal dual-stack/fallback
        // path without requiring IPv6 to be enabled on every Windows host.
        for listener in [
            listener_socket_for(Domain::IPV4, 0).unwrap(),
            listener_socket(0).unwrap(),
        ] {
            let port = listener.local_addr().unwrap().port();
            let competing = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
            competing.set_reuse_address(true).unwrap();
            let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
            let error = competing.bind(&address.into()).unwrap_err();
            assert!(
                matches!(
                    error.kind(),
                    std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied
                ),
                "An exclusive listener must reject a competing IPv4 bind: {error}"
            );
        }
    }
    #[test]
    fn tcp_failures_are_distinct_and_mixed_failures_retain_actionable_cause() {
        let mut failures = Failures::default();
        assert_eq!(failures.error(), Error::TcpConnect);
        failures.record(std::io::ErrorKind::ConnectionRefused);
        assert_eq!(failures.error(), Error::TcpRefused);
        failures.record(std::io::ErrorKind::TimedOut);
        assert_eq!(failures.error(), Error::TcpTimeout);
        failures.record(std::io::ErrorKind::PermissionDenied);
        assert_eq!(failures.error(), Error::TcpPermission);
    }
    #[test]
    fn tls_io_and_certificate_errors_do_not_share_a_network_or_pairing_label() {
        assert_eq!(
            tls_error(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)),
            Error::TlsHandshake
        );
        assert_eq!(
            tls_error(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                rustls::Error::General("pin mismatch".into())
            )),
            Error::Identity
        );
    }
    #[tokio::test]
    async fn real_tcp_success_and_missing_endpoints_are_reported_at_the_correct_stage() {
        assert!(matches!(tcp(vec![]).await, Err(Error::NoEndpoint)));
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let (socket, selected) = tcp(vec![address]).await.unwrap();
        assert_eq!(selected, address);
        assert_eq!(socket.peer_addr().unwrap(), address);
    }
}
