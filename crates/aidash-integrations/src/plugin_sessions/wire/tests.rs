use super::*;
use axum::{Router, body::Body, http::Response, routing::get};
use bytes::Bytes;
use std::convert::Infallible;
use tokio::task::JoinHandle;

async fn stream(
	chunks: Vec<Vec<u8>>,
) -> (BoxStream<'static, Result<Sse, SseError>>, JoinHandle<()>) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let address = listener.local_addr().unwrap();
	let app = Router::new().route(
		"/",
		get(move || {
			let chunks = chunks.clone();
			async move {
				Response::builder()
					.header("content-type", "text/event-stream")
					.body(Body::from_stream(
						futures_util::stream::iter(
							chunks
								.into_iter()
								.map(|chunk| Ok::<_, Infallible>(Bytes::from(chunk))),
						)
						.chain(futures_util::stream::pending()),
					))
					.unwrap()
			}
		}),
	);
	let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	let stream = BoundedClient::new(reqwest::Client::new(), None, Arc::default())
		.get_stream(
			format!("http://{address}/").into(),
			None,
			None,
			None,
			HashMap::new(),
		)
		.await
		.unwrap();
	(stream, server)
}

fn framed_events(newline: &str, split: bool) -> Vec<Vec<u8>> {
	// The optional initial BOM does not turn the first comment into event data.
	let mut chunks = vec![vec![0xef], vec![0xbb], vec![0xbf]];
	// Comment-only keepalives need no blank separator and can continue indefinitely.
	for _ in 0..64 {
		let comment = format!(":keepalive{}", "x".repeat(4096));
		if split {
			chunks.extend(comment.as_bytes().chunks(4093).map(<[u8]>::to_vec));
			chunks.extend(newline.bytes().map(|byte| vec![byte]));
		} else {
			chunks.push(format!("{comment}{newline}").into_bytes());
		}
	}
	for n in 0..4 {
		let event = format!("data: {n}{}", "x".repeat(80_000));
		if split {
			chunks.extend(event.as_bytes().chunks(4093).map(<[u8]>::to_vec));
			// CRLF is one line ending even when its bytes use different HTTP frames.
			chunks.extend(
				newline
					.bytes()
					.chain(newline.bytes())
					.map(|byte| vec![byte]),
			);
		} else {
			chunks.push(format!("{event}{newline}{newline}").into_bytes());
		}
	}
	if split { chunks } else { vec![chunks.concat()] }
}

#[tokio::test]
async fn sse_limit_applies_to_each_event_across_a_long_lived_stream() {
	for newline in ["\n", "\r", "\r\n"] {
		for split in [false, true] {
			let (mut stream, server) = stream(framed_events(newline, split)).await;
			for n in 0..4 {
				let event = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
					.await
					.unwrap()
					.unwrap()
					.unwrap_or_else(|error| panic!("newline {newline:?}, split {split}: {error}"));
				assert_eq!(event.data, Some(format!("{n}{}", "x".repeat(80_000))));
			}
			server.abort();
		}
	}
}

#[tokio::test]
async fn sse_limit_rejects_oversized_events_and_unterminated_fields() {
	for (prefix, newline, fields, terminated) in [
		("data: ", "", 1, false),
		("data: ", "\n", 4, false),
		("data: ", "\r", 4, false),
		("data: ", "\r\n", 4, false),
		(":", "", 1, false),
		(":", "\n", 1, true),
	] {
		let mut chunks = Vec::new();
		for n in 0..fields {
			chunks.push(format!("{prefix}{}", "x".repeat(256_000 / fields + 1)).into_bytes());
			if n + 1 < fields {
				chunks.push(newline.as_bytes().to_vec());
				// Discarded comment lines must not reset the payload of the surrounding event.
				chunks.push(format!(":ping{newline}").into_bytes());
			}
		}
		if terminated {
			chunks.push(newline.as_bytes().to_vec());
		}
		let (mut stream, server) = stream(chunks).await;
		let error = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
			.await
			.unwrap()
			.unwrap()
			.unwrap_err();
		assert!(error.to_string().contains("exceeds 256 KB"), "{error}");
		server.abort();
	}
}
