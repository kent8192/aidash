//! HTTP socket ownership at the native process boundary.
use crate::{Error, Result};
use std::{ffi::OsString, net::SocketAddr};
use tokio::net::TcpListener;

pub(super) async fn bind(address: SocketAddr, inherited: Option<OsString>) -> Result<TcpListener> {
	let Some(inherited) = inherited else {
		return TcpListener::bind(address)
			.await
			.map_err(|error| Error::External(format!("HTTP listener bind {address}: {error}")));
	};
	#[cfg(unix)]
	{
		inherited_listener(address, inherited)
	}
	#[cfg(not(unix))]
	{
		let _ = inherited;
		Err(Error::Invalid("AIDASH_LISTEN_FD requires Unix".into()))
	}
}

#[cfg(unix)]
fn inherited_listener(address: SocketAddr, inherited: OsString) -> Result<TcpListener> {
	use std::os::fd::FromRawFd;
	let fd = inherited
		.to_str()
		.and_then(|value| value.parse::<libc::c_int>().ok())
		.filter(|fd| *fd > libc::STDERR_FILENO)
		.ok_or_else(|| {
			Error::Invalid("AIDASH_LISTEN_FD must be an open descriptor above 2".into())
		})?;
	let diagnostic = |error| {
		Error::External(format!(
			"inherited HTTP listener fd {fd} for {address}: {error}"
		))
	};
	// SAFETY: fcntl only inspects/updates the supplied descriptor. Checking it
	// before taking ownership also makes a closed descriptor a normal error.
	let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
	if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
		return Err(diagnostic(std::io::Error::last_os_error()));
	}
	// SAFETY: the launcher transfers this open descriptor exclusively to the
	// child via AIDASH_LISTEN_FD; no Rust object in the child owns it yet.
	let listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
	let actual = listener.local_addr().map_err(diagnostic)?;
	if actual != address {
		return Err(Error::Invalid(format!(
			"inherited HTTP listener fd {fd} bound to {actual}, expected {address}"
		)));
	}
	if socket_option(fd, libc::SO_TYPE).map_err(diagnostic)? != libc::SOCK_STREAM {
		return Err(Error::Invalid(format!(
			"inherited HTTP listener fd {fd} for {address} is not a stream socket"
		)));
	}
	// Linux exposes listening state through getsockopt; macOS does not support
	// SO_ACCEPTCONN there. Reinhardt's accept loop surfaces invalid state on macOS.
	#[cfg(target_os = "linux")]
	if socket_option(fd, libc::SO_ACCEPTCONN).map_err(diagnostic)? != 1 {
		return Err(Error::Invalid(format!(
			"inherited HTTP listener fd {fd} for {address} is not listening"
		)));
	}
	listener.set_nonblocking(true).map_err(diagnostic)?;
	TcpListener::from_std(listener).map_err(diagnostic)
}

#[cfg(unix)]
fn socket_option(fd: libc::c_int, option: libc::c_int) -> std::io::Result<libc::c_int> {
	let mut value: libc::c_int = 0;
	let mut length = std::mem::size_of_val(&value) as libc::socklen_t;
	// SAFETY: both output pointers refer to live, correctly sized stack values.
	if unsafe {
		libc::getsockopt(
			fd,
			libc::SOL_SOCKET,
			option,
			std::ptr::from_mut(&mut value).cast(),
			&mut length,
		)
	} < 0
	{
		return Err(std::io::Error::last_os_error());
	}
	Ok(value)
}
