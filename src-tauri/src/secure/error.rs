/// Public errors deliberately contain no keys, codes, packet contents or library dumps.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("Bluetooth is unavailable; enable Bluetooth sharing and scan again")]
    BluetoothUnavailable,
    #[error("Bluetooth connection timed out; check the peer and any OS pairing prompt")]
    BluetoothTimeout,
    #[error("Bluetooth RFCOMM connection failed ({0})")]
    BluetoothConnect(String),
    #[error(
        "Local network permission was not granted; LAN discovery and LAN connections are disabled"
    )]
    NetworkPermission,
    #[error("Secure storage is unavailable; no insecure fallback is used")]
    Storage,
    #[error("Identity or trust data is corrupt; existing data was not replaced")]
    Corrupt,
    #[error("This security profile is already in use")]
    Busy,
    #[error("Connection failed or disconnected")]
    Network,
    #[error("No supported LAN endpoint is available; refresh discovery")]
    NoEndpoint,
    #[error("TCP connection timed out; check the advertised port, network and firewall rules")]
    TcpTimeout,
    #[error("TCP connection refused; the peer may have restarted or its advertised port is stale")]
    TcpRefused,
    #[error("TCP access denied by this device; check local-network permissions or VPN policy")]
    TcpPermission,
    #[error("TCP connection failed; check peer addresses, routing and firewall rules")]
    TcpConnect,
    #[error("Invalid INVARDROP_LISTEN_PORT; use a TCP port from 0 to 65535")]
    ListenPort,
    #[error("TCP listener could not bind; check that the configured port is available")]
    ListenBind,
    #[error("TLS handshake failed; no authenticated connection was established")]
    TlsHandshake,
    #[error(
        "TLS handshake timed out; the transport connected but peer verification did not finish"
    )]
    TlsTimeout,
    #[error("Peer authentication timed out after TLS; no secure session was established")]
    AuthenticationTimeout,
    #[error("TLS identity verification failed")]
    Identity,
    #[error("Peer identity has been revoked")]
    Revoked,
    #[error("Pairing was rejected")]
    Rejected,
    #[error("Pairing expired; compare fresh codes on both devices before approving")]
    PairingExpired,
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
    #[error("Saving to Downloads failed; the verified private copy is retained for recovery")]
    Downloads,
    #[error("File could not be read or saved")]
    FileIo,
    #[error("File name is not safe or portable")]
    InvalidName,
    #[error("File size or BLAKE3 verification failed; file was not accepted")]
    Integrity,
    #[error("Source file changed during transfer")]
    FileChanged,
    #[error("Verified file publication has started; cancellation is no longer available")]
    CommitStarted,
}
pub type Result<T> = std::result::Result<T, Error>;
