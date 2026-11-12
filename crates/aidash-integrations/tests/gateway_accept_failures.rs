//! Both gateways outlive a failed accept (#743). Running out of file descriptors
//! is the real way to get one, and it affects the whole process, so this test
//! has a binary to itself and must stay the only test in it.
#![cfg(all(unix, feature = "cloud-adapters"))]

use aidash_application::{Result, egress, preview};
use aidash_integrations::{egress as egress_gateway, preview as preview_gateway};
use async_trait::async_trait;
use rstest::rstest;
use std::{fs::File, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::{TcpListener, TcpSocket, TcpStream},
	sync::watch,
	task::JoinHandle,
};
use uuid::Uuid;

/// Low enough that filling the table is quick, and enough to start a gateway.
const DESCRIPTORS: libc::rlim_t = 256;
/// Longer than the wait between tries, so accept fails more than once.
const OUT_OF_DESCRIPTORS: Duration = Duration::from_millis(1500);

/// The requests here name no Host and no External service, so they are refused
/// before either gateway asks its authority anything.
struct NoAuthority;

#[async_trait]
impl preview::Authority for NoAuthority {
	async fn exchange(&self, _: &str, _: &str) -> Result<preview::Exchange> {
		unreachable!("the request names no Host")
	}
	async fn view(&self, _: &str, _: &str, _: &str) -> Result<preview::Page> {
		unreachable!("the request names no Host")
	}
	async fn live(&self, _: &str, _: &str, _: bool) -> Result<Option<preview::Proxy>> {
		unreachable!("the request names no Host")
	}
	async fn served(&self, _: &str, _: Uuid, _: u64) -> Result<()> {
		unreachable!("the request names no Host")
	}
}

#[async_trait]
impl egress::Authority for NoAuthority {
	async fn admit(
		&self,
		_: &str,
		_: &str,
		_: &str,
		_: &str,
		_: &str,
	) -> Result<egress::Admission> {
		unreachable!("the request names no External service")
	}
}

fn lower_descriptor_limit() {
	let mut limit = libc::rlimit {
		rlim_cur: 0,
		rlim_max: 0,
	};
	// SAFETY: both calls only read or write the local struct passed to them.
	unsafe {
		assert_eq!(libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit), 0);
		limit.rlim_cur = DESCRIPTORS.min(limit.rlim_max);
		assert_eq!(libc::setrlimit(libc::RLIMIT_NOFILE, &limit), 0);
	}
}

/// Open files until the process may open no more, and hold them.
fn fill_descriptor_table() -> Vec<File> {
	let mut held = Vec::new();
	loop {
		match File::open("/dev/null") {
			Ok(file) => held.push(file),
			Err(error) if error.raw_os_error() == Some(libc::EMFILE) => return held,
			Err(error) => panic!("expected to run out of descriptors: {error}"),
		}
	}
}

async fn status_line(mut stream: TcpStream) -> String {
	stream
		.write_all(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n")
		.await
		.unwrap();
	let mut response = Vec::new();
	tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut response))
		.await
		.expect("the gateway should answer")
		.unwrap();
	let response = String::from_utf8_lossy(&response);
	response.lines().next().unwrap_or_default().to_owned()
}

async fn assert_outlives_running_out_of_descriptors(
	name: &str,
	address: SocketAddr,
	gateway: JoinHandle<Result<()>>,
	stop: watch::Sender<bool>,
	refusal: &str,
) {
	// Arrange: an answer means the gateway has opened all it opens at startup.
	let first = TcpStream::connect(address).await.unwrap();
	assert_eq!(status_line(first).await, refusal, "{name}");
	// A socket made now can connect later without needing a new descriptor.
	let socket = TcpSocket::new_v4().unwrap();
	let held = fill_descriptor_table();

	// Act
	let waiting = socket.connect(address).await.unwrap();
	tokio::time::sleep(OUT_OF_DESCRIPTORS).await;

	// Assert: it kept going, and answers again once descriptors are free.
	if gateway.is_finished() {
		panic!("{name} stopped on a failed accept: {:?}", gateway.await);
	}
	drop(held);
	// A failed accept leaves the connection queued on Linux; macOS drops it.
	if cfg!(target_os = "linux") {
		assert_eq!(status_line(waiting).await, refusal, "{name}");
	}
	let after = TcpStream::connect(address).await.unwrap();
	assert_eq!(status_line(after).await, refusal, "{name}");
	stop.send_replace(true);
	gateway.await.unwrap().unwrap();
}

#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_gateways_outlive_running_out_of_descriptors() {
	lower_descriptor_limit();

	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let address = listener.local_addr().unwrap();
	let (stop, stopping) = watch::channel(false);
	let gateway = tokio::spawn(preview_gateway::serve(
		listener,
		Arc::new(NoAuthority),
		stopping,
	));
	assert_outlives_running_out_of_descriptors(
		"Preview gateway",
		address,
		gateway,
		stop,
		"HTTP/1.1 403 Forbidden",
	)
	.await;

	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let address = listener.local_addr().unwrap();
	let (stop, stopping) = watch::channel(false);
	let gateway = tokio::spawn(egress_gateway::serve(
		listener,
		Arc::new(NoAuthority),
		stopping,
	));
	assert_outlives_running_out_of_descriptors(
		"Egress gateway",
		address,
		gateway,
		stop,
		"HTTP/1.1 404 Not Found",
	)
	.await;
}
