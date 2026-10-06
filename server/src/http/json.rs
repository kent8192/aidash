//! Aidash's existing JSON request contract on Reinhardt parameter extraction.
use async_trait::async_trait;
use reinhardt::Request;
use reinhardt::di::params::{
	FromRequest, HasInner, ParamContext, ParamError, ParamErrorContext, ParamResult, ParamType,
};
use serde::de::DeserializeOwned;
use std::ops::Deref;

/// Extract using the existing JSON media, status, and text rejection contract.
#[derive(Debug, Clone)]
pub struct Json<T>(pub T);

/// Keep the serde category through the framework's string-only error context.
/// See docs/design/json-contract-evidence.md. Remove this bridge when native
/// JSON extraction can select the same media/rejection policy and retain the
/// typed category; the ideal replacement is Reinhardt's configured Json<T>.
#[derive(Debug, Clone)]
pub(crate) struct Rejection {
	status: reinhardt::StatusCode,
	message: String,
	envelope: bool,
}

impl Rejection {
	pub(crate) fn response(self) -> reinhardt::Response {
		if self.envelope {
			reinhardt::Response::new(self.status)
				.with_body(br#"{"error":"invalid JSON request"}"#.to_vec())
				.with_header("Content-Type", "application/json")
		} else {
			reinhardt::Response::new(self.status)
				.with_body(self.message.into_bytes())
				.with_header("Content-Type", "text/plain; charset=utf-8")
		}
	}
}

impl<T> Deref for Json<T> {
	type Target = T;
	fn deref(&self) -> &T {
		&self.0
	}
}

impl<T> HasInner for Json<T> {
	type Inner = T;
	fn inner_ref(&self) -> &T {
		&self.0
	}
	fn into_inner(self) -> T {
		self.0
	}
}

#[async_trait]
impl<T: DeserializeOwned + Send> FromRequest for Json<T> {
	async fn from_request(request: &Request, context: &ParamContext) -> ParamResult<Self> {
		extract(request, context, false).await.map(Self)
	}
}

pub(crate) async fn extract<T: DeserializeOwned + Send>(
	request: &Request,
	context: &ParamContext,
	envelope: bool,
) -> ParamResult<T> {
	let media = request
		.headers
		.get(http::header::CONTENT_TYPE)
		.and_then(|value| value.to_str().ok())
		.and_then(|value| value.parse::<mime_guess::mime::Mime>().ok());
	let json_media = media.is_some_and(|media| {
		media.type_() == mime_guess::mime::APPLICATION
			&& (media.subtype() == mime_guess::mime::JSON
				|| media.suffix() == Some(mime_guess::mime::JSON))
	});
	if !json_media {
		request.extensions.insert(Rejection {
			status: reinhardt::StatusCode::UNSUPPORTED_MEDIA_TYPE,
			message: "Expected request with `Content-Type: application/json`".into(),
			envelope,
		});
		return Err(ParamError::InvalidParameter(Box::new(
			ParamErrorContext::new(ParamType::Json, "a JSON content type is required")
				.with_field("Content-Type")
				.with_expected_type::<T>(),
		)));
	}
	// Gateway enforces the route's body limit before extraction. Reuse the
	// native cache so DI and endpoint extraction share one consumption.
	let body = context.read_body_cached(request)?;
	let mut deserializer = serde_json::Deserializer::from_slice(&body);
	let value = serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
		let data = error.inner().classify() == serde_json::error::Category::Data;
		request.extensions.insert(Rejection {
			status: if data {
				reinhardt::StatusCode::UNPROCESSABLE_ENTITY
			} else {
				reinhardt::StatusCode::BAD_REQUEST
			},
			message: format!(
				"{}: {error}",
				if data {
					"Failed to deserialize the JSON body into the target type"
				} else {
					"Failed to parse the request body as JSON"
				}
			),
			envelope,
		});
		// Never attach request bodies to diagnostic context: they can contain
		// passwords, credentials, or private conversation content.
		ParamError::json_deserialization::<T>(error.into_inner(), None)
	})?;
	deserializer.end().map_err(|error| {
		request.extensions.insert(Rejection {
			status: reinhardt::StatusCode::BAD_REQUEST,
			message: format!("Failed to parse the request body as JSON: {error}"),
			envelope,
		});
		ParamError::json_deserialization::<T>(error, None)
	})?;
	Ok(value)
}

#[cfg(test)]
mod tests;
