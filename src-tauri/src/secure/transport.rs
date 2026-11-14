use super::{
    error::{Error, Result},
    identity::{Identity, fingerprint, hex},
};
use rustls::{
    DigitallySignedStruct, DistinguishedName, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
    server::danger::{ClientCertVerified, ClientCertVerifier},
};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

pub const ALPN: &[u8] = b"invardrop/4";
/// Unknown keys are QUARANTINED, not trusted. TLS CertificateVerify signatures
/// remain mandatory. Authority is established by pinned SPKI or out-of-band SAS.
#[derive(Debug)]
struct QuarantinedVerifier {
    expected: Option<String>,
}
impl QuarantinedVerifier {
    fn check(
        &self,
        cert: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
    ) -> std::result::Result<(), rustls::Error> {
        if !chain.is_empty() {
            return Err(rustls::Error::General(
                "Unexpected certificate chain".into(),
            ));
        }
        let id =
            fingerprint(cert).map_err(|_| rustls::Error::General("Invalid device key".into()))?;
        if self
            .expected
            .as_ref()
            .is_some_and(|expected| expected != &id)
        {
            return Err(rustls::Error::General("Pinned device key mismatch".into()));
        }
        Ok(())
    }
    fn signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
}
impl ServerCertVerifier for QuarantinedVerifier {
    fn verify_server_cert(
        &self,
        end: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        self.check(end, chain)?;
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 disabled".into()))
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.signature(message, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ECDSA_NISTP256_SHA256]
    }
}
impl ClientCertVerifier for QuarantinedVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        end: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        self.check(end, chain)?;
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 disabled".into()))
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.signature(message, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ECDSA_NISTP256_SHA256]
    }
}
pub fn acceptor(identity: &Identity) -> Result<TlsAcceptor> {
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|_| Error::Identity)?
    .with_client_cert_verifier(Arc::new(QuarantinedVerifier { expected: None }))
    .with_single_cert(vec![identity.cert()], identity.key())
    .map_err(|_| Error::Identity)?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    config.send_tls13_tickets = 0;
    config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    Ok(TlsAcceptor::from(Arc::new(config)))
}
pub fn connector(identity: &Identity, expected: Option<String>) -> Result<TlsConnector> {
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|_| Error::Identity)?
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(QuarantinedVerifier { expected }))
    .with_client_auth_cert(vec![identity.cert()], identity.key())
    .map_err(|_| Error::Identity)?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    config.enable_sni = false;
    config.resumption = rustls::client::Resumption::disabled();
    Ok(TlsConnector::from(Arc::new(config)))
}
/// Exporter label/context separate this SAS from every other application/purpose.
/// Canonical roles bind both proven TLS keys; names and discovery IDs are absent.
pub fn peer_and_code(
    stream: &TlsStream<TcpStream>,
    local: &str,
    initiator: bool,
) -> Result<(String, String)> {
    let common = stream.get_ref().1;
    if common.alpn_protocol() != Some(ALPN)
        || common.protocol_version() != Some(rustls::ProtocolVersion::TLSv1_3)
    {
        return Err(Error::Protocol);
    }
    let certs = common.peer_certificates().ok_or(Error::Identity)?;
    if certs.len() != 1 {
        return Err(Error::Identity);
    }
    let peer = fingerprint(&certs[0])?;
    if peer == local {
        return Err(Error::Identity);
    }
    let context = if initiator {
        format!("client:{local};server:{peer}")
    } else {
        format!("client:{peer};server:{local}")
    };
    let label = b"EXPORTER-InvarDrop-Pairing-v1";
    let output = match stream {
        TlsStream::Client(s) => {
            s.get_ref()
                .1
                .export_keying_material([0; 16], label, Some(context.as_bytes()))
        }
        TlsStream::Server(s) => {
            s.get_ref()
                .1
                .export_keying_material([0; 16], label, Some(context.as_bytes()))
        }
    }
    .map_err(|_| Error::Identity)?;
    let code = hex(&output)
        .as_bytes()
        .chunks(4)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ");
    Ok((peer, code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn old_transport_alpn_is_rejected_without_fallback() {
        let server = acceptor(&Identity::generate().unwrap()).unwrap();
        let identity = Identity::generate().unwrap();
        let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(QuarantinedVerifier { expected: None }))
        .with_client_auth_cert(vec![identity.cert()], identity.key())
        .unwrap();
        config.alpn_protocols = vec![b"invardrop/3".to_vec()];
        let client = TlsConnector::from(Arc::new(config));
        let (a, b) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move { server.accept(a).await });
        assert!(
            client
                .connect("invardrop.local".try_into().unwrap(), b)
                .await
                .is_err()
        );
        assert!(server.await.unwrap().is_err());
    }
}
