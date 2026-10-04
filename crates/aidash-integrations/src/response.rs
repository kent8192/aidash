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
	while let Some(chunk) = response.chunk().await.map_err(crate::http_error)? {
		if chunk.len() > limit.saturating_sub(bytes.len()) {
			return Err(Error::External(format!("response exceeds {limit} bytes")));
		}
		bytes.extend_from_slice(&chunk);
	}
	serde_json::from_slice(&bytes)
		.map_err(|error| Error::External(format!("invalid response JSON: {error}")))
}
