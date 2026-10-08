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
	use bytes::Bytes;
	use reinhardt::http::ViewResult;
	use reinhardt::test::fixtures::server::TestServerGuard;
	use reinhardt::test::fixtures::{http_client, test_server_guard};
	use reinhardt::{Response, ServerRouter, get};
	use rstest::{fixture, rstest};

	#[get("/large")]
	async fn large() -> ViewResult<Response> {
		Ok(
			Response::ok().with_stream(futures_util::stream::iter((0..8).map(|_| {
				Ok::<_, Box<dyn std::error::Error + Send + Sync>>(Bytes::from(vec![b' '; 64]))
			}))),
		)
	}
	#[get("/invalid")]
	async fn invalid() -> ViewResult<Response> {
		Ok(Response::ok().with_body("not json"))
	}
	#[get("/valid")]
	async fn valid() -> ViewResult<Response> {
		Ok(Response::ok().with_body("{}"))
	}
	#[fixture]
	fn bounded_router() -> ServerRouter {
		ServerRouter::new()
			.endpoint(large)
			.endpoint(invalid)
			.endpoint(valid)
	}
	#[fixture]
	async fn bounded_responses(bounded_router: ServerRouter) -> TestServerGuard {
		// reinhardt-web#6658: the pinned guard is a primitive, without rstest dependency resolution.
		test_server_guard(bounded_router).await
	}
	#[rstest]
	#[tokio::test]
	async fn rejects_oversized_chunked_bodies_and_malformed_json(
		#[future] bounded_responses: TestServerGuard,
		http_client: reqwest::Client,
	) {
		let server = bounded_responses.await;
		for path in ["large", "invalid"] {
			assert!(matches!(
				json::<serde_json::Value>(
					http_client
						.get(format!("{}/{path}", server.url))
						.send()
						.await
						.unwrap(),
					128
				)
				.await,
				Err(Error::External(_))
			));
		}
		assert_eq!(
			json::<serde_json::Value>(
				http_client
					.get(format!("{}/valid", server.url))
					.send()
					.await
					.unwrap(),
				2
			)
			.await
			.unwrap(),
			serde_json::json!({})
		);
	}
}
