use super::*;
use reinhardt::http::ExceptionHandler;
use rstest::{fixture, rstest};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Payload {
	name: String,
}

#[fixture]
fn request(
	#[default("{}")] body: &'static str,
	#[default(Some("application/json"))] content_type: Option<&str>,
) -> Request {
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
	#[case] _body: &'static str,
	#[case] _media: Option<&str>,
	#[case] status: u16,
	#[with(_body, _media)] request: Request,
	context: ParamContext,
) {
	// Arrange
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
async fn json_media_and_cached_extraction_preserve_the_request_contract(
	#[case] _media: &str,
	#[with(r#"{"name":"fixture"}"#, Some(_media))] request: Request,
	context: ParamContext,
) {
	// Arrange
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
#[case::syntax(
	"{",
	Some("application/json"),
	400,
	"Failed to parse the request body as JSON: EOF while parsing an object at line 1 column 1"
)]
#[case::eof(
	"",
	Some("application/json"),
	400,
	"Failed to parse the request body as JSON: EOF while parsing a value at line 1 column 0"
)]
#[case::missing_field(
	"{}",
	Some("application/json"),
	422,
	"Failed to deserialize the JSON body into the target type: missing field `name` at line 1 column 2"
)]
#[case::wrong_type(
	"{\"name\":7}",
	Some("application/json"),
	422,
	"Failed to deserialize the JSON body into the target type: name: invalid type: integer `7`, expected a string at line 1 column 9"
)]
#[case::unknown_field(
	"{\"name\":\"fixture\",\"owner\":\"spoof\"}",
	Some("application/json"),
	422,
	"Failed to deserialize the JSON body into the target type: owner: unknown field `owner`, expected `name` at line 1 column 25"
)]
#[case::trailing_data(
	"{\"name\":\"fixture\"} false",
	Some("application/json"),
	400,
	"Failed to parse the request body as JSON: trailing characters at line 1 column 20"
)]
#[case::missing_media(
	"{\"name\":\"fixture\"}",
	None,
	415,
	"Expected request with `Content-Type: application/json`"
)]
#[case::wrong_media(
	"{\"name\":\"fixture\"}",
	Some("text/plain"),
	415,
	"Expected request with `Content-Type: application/json`"
)]
#[tokio::test]
async fn native_json_rejections_match_the_previous_http_backend(
	#[case] _body: &'static str,
	#[case] _media: Option<&str>,
	#[case] status: u16,
	#[case] diagnostic: &str,
	#[from(request)]
	#[with(_body, _media)]
	native: Request,
	context: ParamContext,
) {
	// Exact status, media and complete diagnostics captured from the previous
	// Axum backend at 015a8a4e; retain all eight independent compatibility cases.
	let error = Json::<Payload>::from_request(&native, &context)
		.await
		.unwrap_err();
	let native = crate::http::ApiErrors
		.handle_exception(&native, error.into())
		.await;
	assert_eq!(native.status.as_u16(), status);
	assert_eq!(
		native.headers[http::header::CONTENT_TYPE],
		"text/plain; charset=utf-8"
	);
	assert_eq!(native.body.as_ref(), diagnostic.as_bytes());
}
#[fixture]
fn context() -> ParamContext {
	ParamContext::new()
}
