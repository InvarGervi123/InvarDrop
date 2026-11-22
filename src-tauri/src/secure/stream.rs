//! Ordered, full-duplex byte streams. Identity and consent belong to TLS and
//! the application protocol, never to the underlying socket or its address.
use tokio::io::{AsyncRead, AsyncWrite};

pub(crate) trait ByteStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> ByteStream for T {}
pub(crate) type Stream = Box<dyn ByteStream>;

#[cfg(any(target_os = "linux", target_os = "android"))]
mod fd {
    use std::{
        io,
        os::fd::{AsRawFd, OwnedFd},
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::{
        io::unix::AsyncFd,
        io::{AsyncRead, AsyncWrite, ReadBuf},
    };

    /// Owns a nonblocking RFCOMM descriptor or an Android ciphertext socketpair.
    pub(crate) struct FdStream(AsyncFd<std::sync::Arc<OwnedFd>>);
    impl FdStream {
        pub(crate) fn new(fd: OwnedFd) -> io::Result<Self> {
            // SAFETY: fd is owned and live for both fcntl calls.
            let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
            if flags < 0
                || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                    < 0
            {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: this owns the descriptor until deregistration. Close
            // handles can only call shutdown, never replace or close the fd.
            Ok(Self(unsafe { AsyncFd::register(std::sync::Arc::new(fd))? }))
        }
    }
    impl FdStream {
        #[cfg(target_os = "linux")]
        pub(crate) fn close_handle(&self) -> std::sync::Weak<OwnedFd> {
            std::sync::Arc::downgrade(self.0.get_ref())
        }
    }
    impl AsyncRead for FdStream {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if buf.remaining() == 0 {
                return Poll::Ready(Ok(()));
            }
            loop {
                let mut ready = std::task::ready!(self.0.poll_read_ready(cx))?;
                match ready.try_io(|fd| {
                    let bytes = buf.initialize_unfilled();
                    // SAFETY: bytes is writable for its length and fd is live.
                    let count = unsafe {
                        libc::recv(
                            fd.get_ref().as_raw_fd(),
                            bytes.as_mut_ptr().cast(),
                            bytes.len(),
                            0,
                        )
                    };
                    if count < 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(count as usize)
                    }
                }) {
                    Ok(Ok(count)) => {
                        buf.advance(count);
                        return Poll::Ready(Ok(()));
                    }
                    Ok(Err(error)) => return Poll::Ready(Err(error)),
                    Err(_) => continue,
                }
            }
        }
    }
    impl AsyncWrite for FdStream {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            loop {
                let mut ready = std::task::ready!(self.0.poll_write_ready(cx))?;
                match ready.try_io(|fd| {
                    // SAFETY: bytes is readable for its length and fd is live.
                    let count = unsafe {
                        libc::send(
                            fd.get_ref().as_raw_fd(),
                            bytes.as_ptr().cast(),
                            bytes.len(),
                            libc::MSG_NOSIGNAL,
                        )
                    };
                    if count < 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(count as usize)
                    }
                }) {
                    Ok(result) => return Poll::Ready(result),
                    Err(_) => continue,
                }
            }
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            // SAFETY: the owned descriptor remains live during shutdown.
            let result = unsafe { libc::shutdown(self.0.get_ref().as_raw_fd(), libc::SHUT_WR) };
            Poll::Ready(if result < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            })
        }
    }
}
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) use fd::FdStream;

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::secure::{
        identity::Identity,
        protocol::{self, Message},
        transport,
    };
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn owned_socket_stream_carries_tls_and_closes_without_leaking_descriptors() {
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let a = FdStream::new(a.into()).unwrap();
        let b = FdStream::new(b.into()).unwrap();
        let close = a.close_handle();
        let server_identity = Identity::generate().unwrap();
        let client_identity = Identity::generate().unwrap();
        let server = transport::acceptor(&server_identity).unwrap();
        let server_pin = server_identity.fingerprint().unwrap();
        let task = tokio::spawn(async move {
            let mut tls: tokio_rustls::TlsStream<_> = server.accept(a).await.unwrap().into();
            assert!(transport::peer_and_code(&tls, &server_pin, false).is_ok());
            assert!(
                matches!(protocol::receive(&mut tls).await.unwrap(), Message::FileChunk { data, .. } if data == vec![0x5a; 64 * 1024])
            );
            protocol::send(&mut tls, &Message::Pong {}).await.unwrap();
        });
        let mut tls = transport::connector(
            &client_identity,
            Some(server_identity.fingerprint().unwrap()),
        )
        .unwrap()
        .connect("invardrop.local".try_into().unwrap(), b)
        .await
        .unwrap();
        protocol::send(
            &mut tls,
            &Message::FileChunk {
                id: uuid::Uuid::new_v4().to_string(),
                data: vec![0x5a; 64 * 1024],
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            protocol::receive(&mut tls).await.unwrap(),
            Message::Pong {}
        ));
        task.await.unwrap();
        assert!(
            close.upgrade().is_none(),
            "A close handle must not keep a closed stream alive"
        );
    }
    #[tokio::test]
    async fn platform_disconnect_interrupts_a_pending_read() {
        use std::os::fd::AsRawFd;
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut a = FdStream::new(a.into()).unwrap();
        let _b = b;
        let close = a.close_handle().upgrade().unwrap();
        let read = tokio::spawn(async move { a.read_u8().await });
        // SAFETY: the strong reference keeps this fd live during shutdown.
        unsafe {
            libc::shutdown(close.as_raw_fd(), libc::SHUT_RDWR);
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), read)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
    }
}
