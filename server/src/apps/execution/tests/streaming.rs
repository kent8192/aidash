use async_trait::async_trait;
use bytes::Bytes;
use futures_util::Stream;
use reinhardt::http::{Handler, ViewResult};
use reinhardt::test::fixtures::server::{TestServerGuard, test_server_guard};
use reinhardt::{Request, Response, ServerRouter};
use rstest::{fixture, rstest};
use std::{
	pin::Pin,
	sync::{
		Arc, Mutex,
		atomic::{AtomicBool, Ordering},
	},
	task::{Context, Poll},
	time::Duration,
};
use tokio::sync::mpsc;

type Chunk = Result<Bytes, Box<dyn std::error::Error + Send + Sync>>;

struct PendingStream {
	receiver: mpsc::Receiver<Chunk>,
	dropped: Arc<AtomicBool>,
}
impl Stream for PendingStream {
	type Item = Chunk;
	fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Chunk>> {
		self.receiver.poll_recv(context)
	}
}
impl Drop for PendingStream {
	fn drop(&mut self) {
		self.dropped.store(true, Ordering::SeqCst);
	}
}

struct StreamingHandler(Mutex<Option<PendingStream>>);
#[async_trait]
impl Handler for StreamingHandler {
	async fn handle(&self, _: Request) -> ViewResult<Response> {
		let stream = self.0.lock().unwrap().take().unwrap();
		Ok(Response::ok()
			.with_stream(Box::pin(stream))
			.with_header("Content-Type", "text/event-stream"))
	}
}

struct StreamingFixture {
	sender: mpsc::Sender<Chunk>,
	dropped: Arc<AtomicBool>,
	server: TestServerGuard,
}

#[fixture]
async fn streaming_server() -> StreamingFixture {
	let (sender, receiver) = mpsc::channel(1);
	let dropped = Arc::new(AtomicBool::new(false));
	let handler = StreamingHandler(Mutex::new(Some(PendingStream {
		receiver,
		dropped: dropped.clone(),
	})));
	let server = test_server_guard(ServerRouter::new().handler("/", handler)).await;
	StreamingFixture {
		sender,
		dropped,
		server,
	}
}

#[rstest]
#[tokio::test]
async fn native_transport_flushes_pending_stream_and_drops_producer_on_disconnect(
	#[future] streaming_server: StreamingFixture,
) {
	// Arrange
	let app = streaming_server.await;
	app.sender
		.send(Ok(Bytes::from_static(b"data: first\n\n")))
		.await
		.unwrap();
	let client = reqwest::Client::new();
	// Act
	let mut response = client.get(&app.server.url).send().await.unwrap();
	let first = tokio::time::timeout(Duration::from_secs(3), response.chunk())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	// Assert
	assert_eq!(first, "data: first\n\n");
	assert_eq!(response.headers()["content-type"], "text/event-stream");
	assert!(!app.dropped.load(Ordering::SeqCst));
	drop(response);
	drop(client);
	tokio::time::timeout(Duration::from_secs(3), async {
		while !app.dropped.load(Ordering::SeqCst) {
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("disconnect must release the pending stream producer");
	assert!(app.sender.is_closed());
}

#[rstest]
#[tokio::test]
async fn native_transport_propagates_stream_errors(#[future] streaming_server: StreamingFixture) {
	// Arrange
	let app = streaming_server.await;
	app.sender
		.send(Ok(Bytes::from_static(b"data: first\n\n")))
		.await
		.unwrap();
	let client = reqwest::Client::new();
	let mut response = client.get(&app.server.url).send().await.unwrap();
	assert_eq!(response.chunk().await.unwrap().unwrap(), "data: first\n\n");
	// Act
	app.sender
		.send(Err(Box::new(std::io::Error::other(
			"fixture stream failure",
		))))
		.await
		.unwrap();
	let result = tokio::time::timeout(Duration::from_secs(3), response.chunk())
		.await
		.unwrap();
	// Assert
	assert!(result.is_err());
}

#[rstest]
#[tokio::test]
async fn dropping_server_guard_releases_an_open_stream(
	#[future] streaming_server: StreamingFixture,
) {
	// Arrange
	let app = streaming_server.await;
	app.sender
		.send(Ok(Bytes::from_static(b"data: first\n\n")))
		.await
		.unwrap();
	let client = reqwest::Client::new();
	let mut response = client.get(&app.server.url).send().await.unwrap();
	assert_eq!(response.chunk().await.unwrap().unwrap(), "data: first\n\n");
	// Act: retain the client connection while dropping only the server guard.
	drop(app.server);
	// Assert
	tokio::time::timeout(Duration::from_secs(3), async {
		while !app.dropped.load(Ordering::SeqCst) {
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("server guard must own and stop accepted connections");
	assert!(app.sender.is_closed());
	let end = tokio::time::timeout(Duration::from_secs(3), response.chunk())
		.await
		.expect("the existing client must observe connection shutdown");
	assert!(!matches!(end, Ok(Some(_))));
}
