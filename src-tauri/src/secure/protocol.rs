use super::error::{Error, Result};
use super::files::CHUNK_SIZE;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME: usize = 2048;
const BINARY: u32 = 1 << 31;
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Message {
    Hello {
        version: u32,
        initiator: bool,
        trusted: bool,
    },
    Decision {
        approve: bool,
    },
    Ready {},
    Ping {},
    Pong {},
    FileOffer {
        id: String,
        name: String,
        size: u64,
    },
    FileAccept {
        id: String,
        accept: bool,
    },
    // Binary payloads never pass through JSON or its unbounded byte-array parser.
    #[serde(skip)]
    FileChunk {
        id: String,
        data: Vec<u8>,
    },
    FileFinish {
        id: String,
        hash: String,
    },
    FileComplete {
        id: String,
    },
}
pub async fn send<W: AsyncWrite + Unpin>(stream: &mut W, message: &Message) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(20), send_frame(stream, message))
        .await
        .map_err(|_| Error::Expired)?
}
async fn send_frame<W: AsyncWrite + Unpin>(stream: &mut W, message: &Message) -> Result<()> {
    if let Message::FileChunk { id, data } = message {
        let uuid = uuid::Uuid::parse_str(id).map_err(|_| Error::Protocol)?;
        if uuid.to_string() != *id || data.is_empty() || data.len() > CHUNK_SIZE {
            return Err(Error::Protocol);
        }
        stream
            .write_u32(BINARY | (16 + data.len()) as u32)
            .await
            .map_err(|_| Error::Network)?;
        stream
            .write_all(uuid.as_bytes())
            .await
            .map_err(|_| Error::Network)?;
        stream.write_all(data).await.map_err(|_| Error::Network)?;
        return stream.flush().await.map_err(|_| Error::Network);
    }
    let bytes = serde_json::to_vec(message).map_err(|_| Error::Protocol)?;
    if bytes.len() > MAX_FRAME {
        return Err(Error::Limit);
    }
    stream
        .write_u32(bytes.len() as u32)
        .await
        .map_err(|_| Error::Network)?;
    stream.write_all(&bytes).await.map_err(|_| Error::Network)?;
    stream.flush().await.map_err(|_| Error::Network)
}
pub async fn receive<R: AsyncRead + Unpin>(stream: &mut R) -> Result<Message> {
    let header = stream.read_u32().await.map_err(|_| Error::Network)?;
    if header & BINARY != 0 {
        let len = (header & !BINARY) as usize;
        if !(17..=16 + CHUNK_SIZE).contains(&len) {
            return Err(Error::Protocol);
        }
        let mut id = [0; 16];
        stream
            .read_exact(&mut id)
            .await
            .map_err(|_| Error::Network)?;
        let mut data = vec![0; len - 16];
        stream
            .read_exact(&mut data)
            .await
            .map_err(|_| Error::Network)?;
        return Ok(Message::FileChunk {
            id: uuid::Uuid::from_bytes(id).to_string(),
            data,
        });
    }
    let len = header as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(Error::Protocol);
    }
    let mut bytes = vec![0; len];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|_| Error::Network)?;
    serde_json::from_slice(&bytes).map_err(|_| Error::Protocol)
}
/// No trust can be committed until both explicit approvals arrive on this channel.
pub struct Pairing {
    deadline: Instant,
    local: bool,
    remote: bool,
    terminal: bool,
}
impl Pairing {
    pub fn new(now: Instant, duration: Duration) -> Self {
        Self {
            deadline: now + duration,
            local: false,
            remote: false,
            terminal: false,
        }
    }
    fn check(&mut self, now: Instant) -> Result<()> {
        if now >= self.deadline {
            self.terminal = true;
            return Err(Error::Expired);
        }
        if self.terminal {
            return Err(Error::Approval);
        }
        Ok(())
    }
    pub fn local(&mut self, approve: bool, code: &str, actual: &str, now: Instant) -> Result<()> {
        self.check(now)?;
        if !approve {
            self.terminal = true;
            return Err(Error::Rejected);
        }
        if self.local || code != actual {
            return Err(Error::Approval);
        }
        self.local = true;
        Ok(())
    }
    pub fn remote(&mut self, approve: bool, now: Instant) -> Result<()> {
        self.check(now)?;
        if !approve {
            self.terminal = true;
            return Err(Error::Rejected);
        }
        if self.remote {
            return Err(Error::Protocol);
        }
        self.remote = true;
        Ok(())
    }
    pub fn approved(&self) -> bool {
        self.local && self.remote && !self.terminal
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_approvals_required_and_wrong_or_duplicate_approval_fails() {
        let now = Instant::now();
        let mut p = Pairing::new(now, Duration::from_secs(90));
        assert!(!p.approved());
        assert!(p.local(true, "wrong", "test-code", now).is_err());
        p.remote(true, now).unwrap();
        assert!(!p.approved());
        p.local(true, "test-code", "test-code", now).unwrap();
        assert!(p.approved());
        assert!(p.local(true, "test-code", "test-code", now).is_err());
    }
    #[test]
    fn rejection_and_expiration_are_terminal() {
        let now = Instant::now();
        let mut p = Pairing::new(now, Duration::from_secs(1));
        assert!(matches!(p.remote(false, now), Err(Error::Rejected)));
        assert!(p.local(true, "x", "x", now).is_err());
        assert!(!p.approved());
        let mut p = Pairing::new(now, Duration::from_secs(1));
        assert!(matches!(
            p.local(true, "x", "x", now + Duration::from_secs(1)),
            Err(Error::Expired)
        ));
        assert!(!p.approved());
    }
    #[tokio::test]
    async fn rejects_oversized_and_malformed_frames() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        a.write_u32(1_000_000).await.unwrap();
        assert!(matches!(receive(&mut b).await, Err(Error::Protocol)));
        a.write_u32(2).await.unwrap();
        a.write_all(b"[]").await.unwrap();
        assert!(receive(&mut b).await.is_err());
        let extra = br#"{"type":"ready","unexpected":true}"#;
        a.write_u32(extra.len() as u32).await.unwrap();
        a.write_all(extra).await.unwrap();
        assert!(receive(&mut b).await.is_err());
    }
    #[tokio::test]
    async fn binary_chunks_are_bounded_and_survive_fragmented_input() {
        let id = uuid::Uuid::new_v4().to_string();
        let expected = vec![0xa7; CHUNK_SIZE];
        let message = Message::FileChunk {
            id: id.clone(),
            data: expected.clone(),
        };
        let (mut a, mut b) = tokio::io::duplex(127);
        let sender = tokio::spawn(async move {
            send(&mut a, &message).await.unwrap();
        });
        match receive(&mut b).await.unwrap() {
            Message::FileChunk { id: got, data } => {
                assert_eq!(got, id);
                assert_eq!(data, expected);
            }
            _ => panic!("Expected binary chunk"),
        }
        sender.await.unwrap();
        for length in [0, 16, CHUNK_SIZE + 17] {
            let (mut a, mut b) = tokio::io::duplex(4);
            a.write_u32(BINARY | length as u32).await.unwrap();
            assert!(matches!(receive(&mut b).await, Err(Error::Protocol)));
        }
    }
    #[tokio::test]
    async fn json_cannot_smuggle_chunks_and_truncated_binary_fails() {
        let data = br#"{"type":"file_chunk","id":"bad","data":[1]}"#;
        let (mut a, mut b) = tokio::io::duplex(256);
        a.write_u32(data.len() as u32).await.unwrap();
        a.write_all(data).await.unwrap();
        assert!(matches!(receive(&mut b).await, Err(Error::Protocol)));
        a.write_u32(BINARY | 18).await.unwrap();
        a.write_all(&[0; 17]).await.unwrap();
        a.shutdown().await.unwrap();
        assert!(matches!(receive(&mut b).await, Err(Error::Network)));
    }
}
