use super::error::{Error, Result};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use rustls::{
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    server::ParsedCertificate,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub certificate: Vec<u8>,
    private_key: Vec<u8>,
}
impl Drop for Identity {
    fn drop(&mut self) {
        self.private_key.zeroize();
    }
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
pub fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| Error::Identity)?;
    Ok(bytes)
}
pub fn fingerprint(cert: &CertificateDer<'_>) -> Result<String> {
    if cert.len() > 2048 {
        return Err(Error::Identity);
    }
    let parsed = ParsedCertificate::try_from(cert).map_err(|_| Error::Identity)?;
    Ok(hex(digest::digest(
        &digest::SHA256,
        parsed.subject_public_key_info().as_ref(),
    )
    .as_ref()))
}
pub fn valid_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl Identity {
    pub fn generate() -> Result<Self> {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["invardrop.local".into()])
                .map_err(|_| Error::Identity)?;
        Ok(Self {
            certificate: cert.der().to_vec(),
            private_key: signing_key.serialize_der(),
        })
    }
    pub fn cert(&self) -> CertificateDer<'static> {
        self.certificate.clone().into()
    }
    pub fn key(&self) -> PrivateKeyDer<'static> {
        PrivatePkcs8KeyDer::from(self.private_key.clone()).into()
    }
    pub fn fingerprint(&self) -> Result<String> {
        fingerprint(&self.cert())
    }
    pub fn validate(&self) -> Result<()> {
        let provider = rustls::crypto::ring::default_provider();
        let key = provider
            .key_provider
            .load_private_key(self.key())
            .map_err(|_| Error::Corrupt)?;
        let certified = rustls::sign::CertifiedKey::new(vec![self.cert()], key);
        certified.keys_match().map_err(|_| Error::Corrupt)?;
        self.fingerprint()?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_is_random_valid_and_not_a_session_uuid() {
        let a = Identity::generate().unwrap();
        let b = Identity::generate().unwrap();
        a.validate().unwrap();
        assert!(valid_fingerprint(&a.fingerprint().unwrap()));
        assert!(a.fingerprint().unwrap() != b.fingerprint().unwrap());
        assert!(fingerprint(&CertificateDer::from(vec![0; 3000])).is_err());
    }
    #[test]
    fn mismatched_private_key_is_rejected() {
        let mut a = Identity::generate().unwrap();
        let b = Identity::generate().unwrap();
        a.private_key = b.private_key.clone();
        assert!(a.validate().is_err());
    }
}
