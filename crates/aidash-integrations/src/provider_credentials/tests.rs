use super::*;
use serde_json::json;
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}
async fn serve(app: axum::Router) -> (String, Server) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}", listener.local_addr().unwrap());
	(
		url,
		Server(tokio::spawn(async move {
			axum::serve(listener, app).await.unwrap();
		})),
	)
}
#[rstest::rstest]
#[case(200, Some(10), false, false)]
#[case(200, None, false, true)]
#[case(401, None, true, false)]
#[case(403, None, true, false)]
#[case(503, None, true, false)]
#[case(429, None, true, false)]
#[tokio::test]
async fn validation_classifies_rejections_and_never_returns_upstream_key_material(
	#[case] status: u16,
	#[case] limit: Option<u64>,
	#[case] rejected: bool,
	#[case] warned: bool,
) {
	use axum::{
		Json, Router,
		http::{HeaderMap, StatusCode},
		routing::get,
	};
	let app = Router::new().route(
		"/key",
		get(move |headers: HeaderMap| async move {
			assert_eq!(headers["authorization"], "Bearer canary-key-for-validation");
			(
				StatusCode::from_u16(status).unwrap(),
				Json(json!({"data":{"limit":limit},"error":"canary-key-for-validation"})),
			)
		}),
	);
	let (url, _server) = serve(app).await;
	let validator = OpenRouterKeyValidator {
		client: reqwest::Client::builder()
			.redirect(reqwest::redirect::Policy::none())
			.build()
			.unwrap(),
	};
	let result = validator
		.validate_at(
			&format!("{url}/key"),
			&SecretString::from("canary-key-for-validation"),
		)
		.await;
	assert_eq!(result.is_err(), rejected);
	match result {
		Ok(v) => assert_eq!(!v.warnings.is_empty(), warned),
		Err(e) => {
			assert!(!e.to_string().contains("canary-key-for-validation"));
			if status == 503 || status == 429 {
				assert!(matches!(e, Error::ProviderRejected { status: 503, .. }));
			} else {
				assert!(matches!(e, Error::Invalid(_)));
			}
		}
	}
}
