//! Bounded decoding at external HTTP trust boundaries.
use crate::{Error, Result};
use serde::de::DeserializeOwned;

pub(crate) async fn json<T: DeserializeOwned>(
	mut response: reqwest::Response,
	limit: usize,
) -> Result<T> {
	if response
		.content_length()
		.is_some_and(|length| length > limit as u64)
	{
		return Err(Error::External(format!("response exceeds {limit} bytes")));
	}
	let mut bytes = Vec::new();
	while let Some(chunk) = response.chunk().await? {
		if chunk.len() > limit.saturating_sub(bytes.len()) {
			return Err(Error::External(format!("response exceeds {limit} bytes")));
		}
		bytes.extend_from_slice(&chunk);
	}
	serde_json::from_slice(&bytes)
		.map_err(|error| Error::External(format!("invalid response JSON: {error}")))
}

#[cfg(test)]
mod tests {
	use super::*;
	use axum::{Router, body::Body, routing::get};

	#[rstest::rstest]
	#[tokio::test]
	async fn rejects_oversized_chunked_bodies_and_malformed_json() {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let url = format!("http://{}", listener.local_addr().unwrap());
		let server = tokio::spawn(async move {
			axum::serve(
				listener,
				Router::new()
					.route(
						"/large",
						get(|| async {
							Body::from_stream(futures_util::stream::iter(
								(0..8).map(|_| Ok::<_, std::io::Error>(vec![b' '; 64])),
							))
						}),
					)
					.route("/invalid", get(|| async { "not json" }))
					.route("/valid", get(|| async { "{}" })),
			)
			.await
			.unwrap();
		});
		let client = reqwest::Client::new();
		for path in ["large", "invalid"] {
			assert!(matches!(
				json::<serde_json::Value>(
					client.get(format!("{url}/{path}")).send().await.unwrap(),
					128
				)
				.await,
				Err(Error::External(_))
			));
		}
		assert_eq!(
			json::<serde_json::Value>(client.get(format!("{url}/valid")).send().await.unwrap(), 2)
				.await
				.unwrap(),
			serde_json::json!({})
		);
		server.abort();
	}
}
