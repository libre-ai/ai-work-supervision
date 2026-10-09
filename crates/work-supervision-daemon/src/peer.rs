use std::os::unix::net::UnixStream;

use crate::Failure;

/// Effective user id of the process at the other end of `stream`, from the kernel.
///
/// # Errors
///
/// `socket.peer_unknown` when the kernel does not report it.
pub fn peer_uid(stream: &UnixStream) -> Result<u32, Failure> {
    platform_peer_uid(stream).map_err(|_| Failure::new("socket.peer_unknown"))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn platform_peer_uid(stream: &UnixStream) -> nix::Result<u32> {
    let credentials =
        nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::PeerCredentials)?;
    Ok(credentials.uid())
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn platform_peer_uid(stream: &UnixStream) -> nix::Result<u32> {
    let (uid, _) = nix::unistd::getpeereid(stream)?;
    Ok(uid.as_raw())
}

/// Admits only a peer running as the daemon's own effective user.
///
/// The socket is already mode 0600 inside a 0700 directory; this is the
/// second, independent check.
///
/// # Errors
///
/// `socket.peer_refused`.
pub const fn authorize_peer(peer: u32, own: u32) -> Result<(), Failure> {
    if peer == own {
        Ok(())
    } else {
        Err(Failure::new("socket.peer_refused"))
    }
}
