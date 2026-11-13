/// Public errors deliberately contain no keys, codes, packet contents or library dumps.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("Secure storage is unavailable; no insecure fallback is used")]
    Storage,
    #[error("Identity or trust data is corrupt; existing data was not replaced")]
    Corrupt,
    #[error("This security profile is already in use")]
    Busy,
    #[error("Connection failed or disconnected")]
    Network,
    #[error("TLS identity verification failed")]
    Identity,
    #[error("Peer identity has been revoked")]
    Revoked,
    #[error("Pairing was rejected")]
    Rejected,
    #[error("Request expired")]
    Expired,
    #[error("Connection cancelled")]
    Cancelled,
    #[error("Invalid or unexpected protocol message")]
    Protocol,
    #[error("Invalid or stale approval")]
    Approval,
    #[error("Connection or storage limit reached")]
    Limit,
    #[error("Service is not ready")]
    NotReady,
}
pub type Result<T> = std::result::Result<T, Error>;
