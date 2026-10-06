use super::*;
use axum::{
	Json, Router,
	extract::Path,
	http::{HeaderMap, StatusCode},
	response::IntoResponse,
	routing::{get, post},
};
use rstest::rstest;
use std::sync::RwLock;
struct Keys(RwLock<Option<String>>);
impl Credentials for Keys {
	fn resolve(&self, reference: &str) -> Result<String> {
		assert_eq!(reference, "RUNNER_KEY");
		self.0
			.read()
			.unwrap()
			.clone()
			.ok_or_else(|| Error::Invalid("fixture unavailable".into()))
	}
}
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}
async fn fixture() -> (Server, RunnerHttp, Arc<Keys>) {
	let app = Router::new()
		.route(
			"/reply",
			post(|headers: HeaderMap, Json(body): Json<Value>| async move {
				Json(
					json!({"authorization":headers["authorization"].to_str().unwrap(),"body":body}),
				)
			}),
		)
		.route(
			"/v1/operations/{id}",
			get(|| async { (StatusCode::NOT_FOUND, Json(json!({"error":"missing"}))) }),
		)
		.route(
			"/v1/operations",
			get(|| async { (StatusCode::NOT_FOUND, Json(json!({"error":"missing"}))) }),
		)
		.route(
			"/error/{status}",
			get(|Path(status): Path<u16>| async move {
				(
					StatusCode::from_u16(status).unwrap(),
					Json(json!({"error":"fixture error"})),
				)
			}),
		)
		.route(
			"/redirect",
			get(|| async {
				(
					StatusCode::TEMPORARY_REDIRECT,
					[("location", "/v1/operations/one")],
					Json(json!({"error":"redirect rejected"})),
				)
					.into_response()
			}),
		)
		.route(
			"/v1/operations/invalid-json",
			get(|| async { (StatusCode::NOT_FOUND, "not-json") }),
		);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/", listener.local_addr().unwrap());
	let server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap()
	}));
	let keys = Arc::new(Keys(RwLock::new(Some("before-rotation".into()))));
	let client = RunnerHttp {
		endpoint,
		credential_env: "RUNNER_KEY".into(),
		credentials: keys.clone(),
	};
	(server, client, keys)
}
#[rstest]
#[tokio::test]
async fn payload_bearer_header_and_per_request_credential_rotation_are_preserved() {
	let (_server, client, keys) = fixture().await;
	let first = client
		.request("POST", "/reply", Some(&json!({"revision":1})))
		.await
		.unwrap();
	*keys.0.write().unwrap() = Some("after-rotation".into());
	let second = client
		.request("POST", "/reply", Some(&json!({"revision":2})))
		.await
		.unwrap();
	assert_eq!(
		first,
		json!({"authorization":"Bearer before-rotation","body":{"revision":1}})
	);
	assert_eq!(
		second,
		json!({"authorization":"Bearer after-rotation","body":{"revision":2}})
	);
}
#[rstest]
#[tokio::test]
async fn a_missing_operation_is_distinct_from_a_missing_collection() {
	let (_server, client, _keys) = fixture().await;
	assert_eq!(
		client
			.request("GET", "/v1/operations/one", None)
			.await
			.unwrap(),
		json!({"status":"absent"})
	);
	assert!(
		matches!(client.request("GET","/v1/operations",None).await,Err(Error::External(message)) if message=="runner 404: \"missing\"")
	);
}
#[rstest]
#[case::bad_request(400)]
#[case::server_failure(500)]
#[tokio::test]
async fn runner_error_status_and_json_error_text_are_preserved(#[case] status: u16) {
	let (_server, client, _keys) = fixture().await;
	assert!(
		matches!(client.request("GET",&format!("/error/{status}"),None).await,Err(Error::External(message)) if message==format!("runner {status}: \"fixture error\""))
	);
}
#[rstest]
#[tokio::test]
async fn redirects_are_rejected_without_forwarding_credentials_to_another_endpoint() {
	let (_server, client, _keys) = fixture().await;
	assert!(
		matches!(client.request("GET","/redirect",None).await,Err(Error::External(message)) if message=="runner 307: \"redirect rejected\"")
	);
}
#[rstest]
#[tokio::test]
async fn invalid_json_does_not_masquerade_as_an_absent_operation() {
	let (_server, client, _keys) = fixture().await;
	assert!(matches!(
		client
			.request("GET", "/v1/operations/invalid-json", None)
			.await,
		Err(Error::External(_))
	));
}
#[rstest]
#[tokio::test]
async fn missing_credentials_keep_the_existing_runtime_unavailable_error() {
	let (_server, client, keys) = fixture().await;
	*keys.0.write().unwrap() = None;
	assert!(
		matches!(client.request("GET","/v1/health",None).await,Err(Error::Conflict(message)) if message=="RUNTIME_UNAVAILABLE: runner credential unavailable")
	);
}
#[rstest]
#[case::remote_http("http://192.0.2.1")]
#[case::hostname_http("http://localhost")]
#[case::ftp("ftp://127.0.0.1")]
#[tokio::test]
async fn non_tls_non_loopback_endpoints_are_rejected_before_any_request(#[case] endpoint: &str) {
	let client = RunnerHttp {
		endpoint: endpoint.into(),
		credential_env: "RUNNER_KEY".into(),
		credentials: Arc::new(Keys(RwLock::new(None))),
	};
	assert!(
		matches!(client.request("GET","/v1/health",None).await,Err(Error::Invalid(message)) if message=="runner transport requires TLS or loopback")
	);
}
