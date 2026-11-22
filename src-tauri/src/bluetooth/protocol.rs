//! Small unauthenticated bootstrap and routing preface, never credentials.
use crate::secure::error::{Error, Result};
use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "linux", test))]
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub(super) const MAX_BOOTSTRAP: usize = 512;
#[cfg(any(target_os = "linux", test))]
const MAGIC: &[u8; 8] = b"IVDBT001";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Bootstrap {
    pub version: u8,
    pub id: String,
    pub name: String,
    pub service: String,
    pub address: Option<String>,
}
impl Bootstrap {
    pub fn validate(&self) -> Result<()> {
        let run = uuid::Uuid::parse_str(&self.id).map_err(|_| Error::Protocol)?;
        let service = uuid::Uuid::parse_str(&self.service).map_err(|_| Error::Protocol)?;
        if self.version != 1 || run.get_version_num() != 4 || service.get_version_num() != 4
            || run.to_string() != self.id || service.to_string() != self.service
            || self.name.is_empty() || self.name.len() > 80
            || self.name.chars().any(|c| c.is_control() || matches!(c, '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
            || self.address.as_ref().is_some_and(|a| !valid_address(a)) { return Err(Error::Protocol); }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| Error::Protocol)?;
        if bytes.len() > MAX_BOOTSTRAP {
            return Err(Error::Limit);
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_BOOTSTRAP {
            return Err(Error::Limit);
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| Error::Protocol)?;
        value.validate()?;
        Ok(value)
    }
}
pub(super) fn valid_address(value: &str) -> bool {
    value.len() == 17
        && value.bytes().enumerate().all(|(i, b)| {
            if i % 3 == 2 {
                b == b':'
            } else {
                b.is_ascii_hexdigit()
            }
        })
        && value != "00:00:00:00:00:00"
        && value != "02:00:00:00:00:00"
}
#[cfg(any(target_os = "linux", test))]
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReverseRequest {
    pub version: u8,
    pub address: String,
    pub service: String,
    pub token: String,
}
#[cfg(any(target_os = "linux", test))]
impl ReverseRequest {
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.version != 1
            || !valid_address(&self.address)
            || uuid::Uuid::parse_str(&self.service).is_err()
            || uuid::Uuid::parse_str(&self.token).is_err()
        {
            return Err(Error::Protocol);
        }
        serde_json::to_vec(self).map_err(|_| Error::Protocol)
    }
}

#[cfg(any(target_os = "linux", test))]
pub(super) async fn preface<W: AsyncWrite + Unpin>(
    stream: &mut W,
    token: Option<uuid::Uuid>,
) -> Result<()> {
    stream.write_all(MAGIC).await.map_err(|_| Error::Network)?;
    stream
        .write_u8(u8::from(token.is_some()))
        .await
        .map_err(|_| Error::Network)?;
    stream
        .write_all(token.unwrap_or(uuid::Uuid::nil()).as_bytes())
        .await
        .map_err(|_| Error::Network)?;
    stream.flush().await.map_err(|_| Error::Network)
}
#[cfg(any(target_os = "linux", test))]
pub(super) async fn read_preface<R: AsyncRead + Unpin>(
    stream: &mut R,
) -> Result<Option<uuid::Uuid>> {
    let mut bytes = [0; 25];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|_| Error::Network)?;
    if &bytes[..8] != MAGIC {
        return Err(Error::Protocol);
    }
    let token = uuid::Uuid::from_slice(&bytes[9..]).map_err(|_| Error::Protocol)?;
    match (bytes[8], token.is_nil()) {
        (0, true) => Ok(None),
        (1, false) => Ok(Some(token)),
        _ => Err(Error::Protocol),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn bootstrap() -> Bootstrap {
        Bootstrap {
            version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            name: "InvarDrop nearby".into(),
            service: uuid::Uuid::new_v4().to_string(),
            address: None,
        }
    }
    #[test]
    fn bootstrap_is_bounded_and_never_accepts_invalid_locators_or_names() {
        let original = bootstrap();
        assert_eq!(
            Bootstrap::decode(&original.encode().unwrap()).unwrap().id,
            original.id
        );
        for name in ["".into(), "x".repeat(81), "spoof\u{202e}".into()] {
            let mut value = original.clone();
            value.name = name;
            assert!(value.encode().is_err());
        }
        for address in [
            "00:00:00:00:00:00",
            "02:00:00:00:00:00",
            "127.0.0.1",
            "AA:BB:CC:DD:EE",
            "GG:BB:CC:DD:EE:FF",
        ] {
            let mut value = original.clone();
            value.address = Some(address.into());
            assert!(value.encode().is_err());
        }
        assert!(valid_address("AA:BB:CC:DD:EE:FF"));
        assert!(Bootstrap::decode(&vec![0; MAX_BOOTSTRAP + 1]).is_err());
        let mut json = serde_json::to_value(original).unwrap();
        json["trusted"] = true.into();
        assert!(Bootstrap::decode(&serde_json::to_vec(&json).unwrap()).is_err());
    }
    #[tokio::test]
    async fn fragmented_prefaces_correlate_only_valid_one_use_tokens() {
        for token in [None, Some(uuid::Uuid::new_v4())] {
            let (mut a, mut b) = tokio::io::duplex(1);
            let writer = tokio::spawn(async move {
                preface(&mut a, token).await.unwrap();
            });
            assert_eq!(read_preface(&mut b).await.unwrap(), token);
            writer.await.unwrap();
        }
        for (role, token) in [
            (0, uuid::Uuid::new_v4()),
            (1, uuid::Uuid::nil()),
            (2, uuid::Uuid::new_v4()),
        ] {
            let (mut a, mut b) = tokio::io::duplex(25);
            a.write_all(MAGIC).await.unwrap();
            a.write_u8(role).await.unwrap();
            a.write_all(token.as_bytes()).await.unwrap();
            assert_eq!(read_preface(&mut b).await.unwrap_err(), Error::Protocol);
        }
    }
}
