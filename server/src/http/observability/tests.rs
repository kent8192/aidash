use super::*;
use bytes::Bytes;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use rstest::rstest;
use std::{
	io::Write,
	sync::{Arc, Mutex},
};

fn request(method: &str, uri: &str) -> Request {
	Request::builder()
		.method(method.parse().unwrap())
		.uri(uri)
		.body(Bytes::new())
		.build()
		.unwrap()
}
fn pending(handle: &PrometheusHandle, expected: u8) {
	let rendered = handle.render();
	let line = rendered
		.lines()
		.find(|line| line.starts_with("axum_http_requests_pending{"))
		.expect("pending request series");
	assert!(line.ends_with(&format!(" {expected}")), "{line}");
	assert!(line.contains("method=\"GET\""));
	assert!(line.contains("endpoint=\"/api/events/stream\""));
}

#[rstest]
#[case::parameter(
	"GET",
	"/api/workspaces/7bb1b620-7b40-4bf5-b5b0-0a59f0c29ffb?token=private",
	"GET",
	"/api/workspaces/{id}"
)]
#[case::static_route("POST", "/api/workspaces?token=private", "POST", "/api/workspaces")]
#[case::slash_fallback("POST", "/api/workspaces/", "POST", "/api/workspaces")]
#[case::unmatched("POST", "/unknown/private-value", "POST", "unmatched")]
#[case::unknown_method("PRIVATE_METHOD", "/api/workspaces", "UNKNOWN", "unmatched")]
#[case::asset("GET", "/assets/private-filename.js", "GET", "/{<path:asset>}")]
fn request_labels_only_contain_registered_templates_and_bounded_methods(
	#[case] method: &str,
	#[case] uri: &str,
	#[case] expected_method: &str,
	#[case] expected_route: &str,
) {
	// Arrange / Act: metadata comes from the production Reinhardt router.
	let request = request(method, uri);
	// Assert
	assert_eq!(method_label(&request.method), expected_method);
	assert_eq!(route_label(&request), expected_route);
}

#[rstest]
#[tokio::test]
async fn pending_owns_unpolled_and_completed_stream_lifetimes() {
	// Arrange: a thread-local recorder avoids modifying process-global telemetry.
	let recorder = PrometheusBuilder::new().build_recorder();
	let handle = recorder.handle();
	let _recorder = metrics::set_default_local_recorder(&recorder);
	let observation = Observation::start(&request("GET", "/api/events/stream?token=private"));
	let stream = futures_util::stream::iter([Ok(Bytes::from_static(b"data: event\n\n"))]);
	// Act
	let mut response = observation.finish(Response::ok().with_stream(stream), "request-fixture");
	pending(&handle, 1);
	let mut body = response.take_stream_body().unwrap();
	drop(response);
	assert_eq!(body.next().await.unwrap().unwrap(), "data: event\n\n");
	pending(&handle, 1);
	assert!(body.next().await.is_none());
	// Assert: completion releases the gauge even if the stream object survives.
	pending(&handle, 0);
	let metrics = handle.render();
	assert!(
		metrics
			.lines()
			.any(|line| line.starts_with("axum_http_requests_total{")
				&& line.contains("status=\"200\"")
				&& line.ends_with(" 1"))
	);
	assert!(metrics.lines().any(|line| {
		line.starts_with("axum_http_requests_duration_seconds_count{")
			&& line.contains("endpoint=\"/api/events/stream\"")
			&& line.ends_with(" 1")
	}));
	assert!(!metrics.contains("private"));

	let response = Observation::start(&request("GET", "/api/events/stream")).finish(
		Response::ok().with_stream(futures_util::stream::pending()),
		"unpolled-fixture",
	);
	pending(&handle, 1);
	drop(response);
	pending(&handle, 0);
}

#[rstest]
#[tokio::test]
async fn cancellations_and_body_errors_release_pending() {
	// Arrange
	let recorder = PrometheusBuilder::new().build_recorder();
	let handle = recorder.handle();
	let _recorder = metrics::set_default_local_recorder(&recorder);
	// Act / Assert: cancelled handler futures drop their observation.
	let observation = Observation::start(&request("GET", "/api/events/stream"));
	pending(&handle, 1);
	drop(observation);
	pending(&handle, 0);
	let failed =
		futures_util::stream::iter([Err(Box::new(std::io::Error::other("fixture failure"))
			as Box<dyn std::error::Error + Send + Sync>)]);
	let mut response = Observation::start(&request("GET", "/api/events/stream"))
		.finish(Response::ok().with_stream(failed), "failed-fixture");
	let mut body = response.take_stream_body().unwrap();
	assert!(body.next().await.unwrap().is_err());
	pending(&handle, 0);
}

#[rstest]
#[tokio::test]
async fn pending_preserves_buffered_and_file_range_bytes_and_framing() {
	// Arrange
	let recorder = PrometheusBuilder::new().build_recorder();
	let handle = recorder.handle();
	let _recorder = metrics::set_default_local_recorder(&recorder);
	let response =
		Response::new(reinhardt::StatusCode::SERVICE_UNAVAILABLE).with_body("overloaded");
	// Act
	let mut response =
		Observation::start(&request("GET", "/api/events/stream")).finish(response, "error-fixture");
	assert_eq!(response.headers["content-length"], "10");
	let mut body = response.take_stream_body().unwrap();
	assert_eq!(body.next().await.unwrap().unwrap(), "overloaded");
	assert!(body.next().await.is_none());
	pending(&handle, 0);
	let response = Observation::start(&request("GET", "/api/events/stream"))
		.finish(Response::no_content(), "bodyless-fixture");
	assert!(!response.headers.contains_key("content-length"));
	drop(response);
	pending(&handle, 0);

	let mut file = tempfile::tempfile().unwrap();
	let bytes: Vec<_> = (0..150_000).map(|i| (i % 251) as u8).collect();
	file.write_all(&bytes).unwrap();
	let response = Response::new(reinhardt::StatusCode::PARTIAL_CONTENT)
		.with_file_body(file, 123, 131_072)
		.unwrap()
		.with_header("Content-Range", "bytes 123-131194/150000");
	let mut response =
		Observation::start(&request("GET", "/api/events/stream")).finish(response, "file-fixture");
	// Assert: the file remains an owned range streamed in bounded chunks.
	assert_eq!(response.headers["content-length"], "131072");
	assert_eq!(response.headers["content-range"], "bytes 123-131194/150000");
	let mut body = response.take_stream_body().unwrap();
	let mut received = Vec::new();
	while let Some(chunk) = body.next().await {
		let chunk = chunk.unwrap();
		assert!(chunk.len() <= 65_536);
		received.extend_from_slice(&chunk);
	}
	assert_eq!(received, bytes[123..131_195]);
	pending(&handle, 0);
}

#[derive(Clone)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);
impl Write for LogBuffer {
	fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
		self.0.lock().unwrap().extend_from_slice(bytes);
		Ok(bytes.len())
	}
	fn flush(&mut self) -> std::io::Result<()> {
		Ok(())
	}
}
#[rstest]
fn response_log_has_method_and_route_without_raw_request_values() {
	// Arrange
	let buffer = LogBuffer(Arc::new(Mutex::new(Vec::new())));
	let writer = buffer.clone();
	let subscriber = tracing_subscriber::fmt()
		.without_time()
		.with_ansi(false)
		.with_writer(move || writer.clone())
		.finish();
	let _subscriber = tracing::subscriber::set_default(subscriber);
	// Act
	let response = Observation::start(&request(
		"GET",
		"/api/workspaces/7bb1b620-7b40-4bf5-b5b0-0a59f0c29ffb?token=private",
	))
	.finish(Response::not_found(), "request-fixture");
	drop(response);
	// Assert
	let log = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
	for field in [
		"HTTP response",
		"method=\"GET\"",
		"route=\"/api/workspaces/{id}\"",
		"status=404",
		"duration_ms=",
		"request-fixture",
	] {
		assert!(log.contains(field), "{log}");
	}
	for value in ["7bb1b620", "token", "private"] {
		assert!(!log.contains(value), "{log}");
	}
}
