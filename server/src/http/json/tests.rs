use super::*;
use reinhardt::http::ExceptionHandler;
use rstest::rstest;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Payload {
	name: String,
}

fn request(body: &'static str, content_type: Option<&str>) -> Request {
	let mut headers = http::HeaderMap::new();
	if let Some(content_type) = content_type {
		headers.insert(http::header::CONTENT_TYPE, content_type.parse().unwrap());
	}
	Request::builder()
		.method(http::Method::POST)
		.uri("/fixture")
		.headers(headers)
		.body(bytes::Bytes::from_static(body.as_bytes()))
		.build()
		.unwrap()
}

#[rstest]
#[case::syntax("{", Some("application/json"), 400)]
#[case::eof("", Some("application/json"), 400)]
#[case::missing_field("{}", Some("application/json"), 422)]
#[case::wrong_type(r#"{"name":7}"#, Some("application/json"), 422)]
#[case::unknown_field(r#"{"name":"private","owner":"spoof"}"#, Some("application/json"), 422)]
#[case::no_media(r#"{"name":"private"}"#, None, 415)]
#[case::plain_text(r#"{"name":"private"}"#, Some("text/plain"), 415)]
#[case::substring(r#"{"name":"private"}"#, Some("text/application/json"), 415)]
#[tokio::test]
async fn json_rejections_keep_status_and_safe_envelope(
	#[case] body: &'static str,
	#[case] media: Option<&str>,
	#[case] status: u16,
) {
	// Arrange
	let request = request(body, media);
	let context = ParamContext::new();
	// Act
	let error = super::extract::<Payload>(&request, &context, true)
		.await
		.unwrap_err();
	assert_eq!(error.context().unwrap().raw_value, None);
	let response = crate::http::ApiErrors
		.handle_exception(&request, error.into())
		.await;
	// Assert
	assert_eq!(response.status.as_u16(), status);
	assert_eq!(
		serde_json::from_slice::<serde_json::Value>(&response.body).unwrap(),
		json!({"error":"invalid JSON request"})
	);
}

#[rstest]
#[case("application/json")]
#[case("application/json; charset=utf-8")]
#[case("application/problem+json")]
#[tokio::test]
async fn json_media_and_cached_extraction_preserve_the_request_contract(#[case] media: &str) {
	// Arrange
	let request = request(r#"{"name":"fixture"}"#, Some(media));
	let context = ParamContext::new();
	// Act: a second extractor must reuse the framework cache.
	let first = Json::<Payload>::from_request(&request, &context)
		.await
		.unwrap();
	let second = Json::<Payload>::from_request(&request, &context)
		.await
		.unwrap();
	// Assert
	assert_eq!(first.0, second.0);
	assert_eq!(first.name, "fixture");
	assert!(request.extensions.get::<Rejection>().is_none());
}

#[rstest]
#[case::syntax("{", Some("application/json"))]
#[case::eof("", Some("application/json"))]
#[case::missing_field("{}", Some("application/json"))]
#[case::wrong_type(r#"{"name":7}"#, Some("application/json"))]
#[case::unknown_field(r#"{"name":"fixture","owner":"spoof"}"#, Some("application/json"))]
#[case::trailing_data(r#"{"name":"fixture"} false"#, Some("application/json"))]
#[case::missing_media(r#"{"name":"fixture"}"#, None)]
#[case::wrong_media(r#"{"name":"fixture"}"#, Some("text/plain"))]
#[tokio::test]
async fn native_json_rejections_match_the_previous_http_backend(
	#[case] body: &'static str,
	#[case] media: Option<&str>,
) {
	use axum::{extract::FromRequest as _, response::IntoResponse};
	// Arrange: Axum is a test-only compatibility oracle, outside production.
	let native = request(body, media);
	let mut previous = http::Request::builder()
		.method(http::Method::POST)
		.uri("/fixture");
	if let Some(media) = media {
		previous = previous.header(http::header::CONTENT_TYPE, media);
	}
	let previous = previous.body(axum::body::Body::from(body)).unwrap();
	// Act
	let error = Json::<Payload>::from_request(&native, &ParamContext::new())
		.await
		.unwrap_err();
	let native = crate::http::ApiErrors
		.handle_exception(&native, error.into())
		.await;
	let previous = axum::Json::<Payload>::from_request(previous, &())
		.await
		.unwrap_err()
		.into_response();
	let (parts, previous) = previous.into_parts();
	let previous = axum::body::to_bytes(previous, 4096).await.unwrap();
	// Assert: status, media and complete diagnostic text preserve the contract.
	assert_eq!(native.status, parts.status);
	assert_eq!(
		native.headers[http::header::CONTENT_TYPE],
		parts.headers[http::header::CONTENT_TYPE]
	);
	assert_eq!(native.body.as_ref(), previous.as_ref());
}
