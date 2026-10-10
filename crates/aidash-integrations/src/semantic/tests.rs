use super::*;
use axum::{Json, Router, http::HeaderMap, routing::post};
use rstest::rstest;

struct FixtureCredentials;
impl aidash_application::ports::Credentials for FixtureCredentials {
	fn resolve(&self, name: &str) -> Result<String> {
		assert_eq!(name, "AIDASH_SECRET_OPENROUTER");
		Ok("fixture-key".into())
	}
}

struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}

fn configuration(endpoint: String) -> EmbeddingConfig {
	EmbeddingConfig {
		provider: "openrouter".into(),
		endpoint,
		credential_env: Some("AIDASH_SECRET_OPENROUTER".into()),
		provider_credential: None,
		model: "google/gemini-embedding-2".into(),
		model_version: "1.0.0".into(),
		dimensions: 3072,
	}
}

#[rstest]
#[case(3072)]
#[case(1536)]
#[tokio::test]
async fn openrouter_gemini_sends_approved_dimensions_and_bearer_credentials(
	#[case] dimensions: usize,
) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/api/v1", listener.local_addr().unwrap());
	let input_text = "日本語の知見 / Multilingual findings";
	let app = Router::new().route(
		"/api/v1/embeddings",
		post(
			move |headers: HeaderMap, Json(body): Json<Value>| async move {
				assert_eq!(headers["authorization"], "Bearer fixture-key");
				assert_eq!(
					body,
					json!({
						"model":"google/gemini-embedding-2", "input":input_text,
						"dimensions":dimensions, "encoding_format":"float",
						"provider":{"zdr":true}
					})
				);
				let mut vector = vec![0.0; dimensions];
				vector[0] = 1.0;
				Json(
					json!({"model":body["model"],"data":[{"index":0,"embedding":vector}],
				"usage":{"prompt_tokens":12,"total_tokens":12}}),
				)
			},
		),
	);
	let _server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	}));
	let mut config = configuration(endpoint);
	config.dimensions = dimensions;
	let result = embed(
		&aidash_application::provider_access::EnvironmentAccess {
			credentials: Arc::new(FixtureCredentials),
		},
		&Context::default(),
		&client().unwrap(),
		&config,
		input_text,
	)
	.await
	.unwrap();
	assert_eq!(result.vector.len(), dimensions);
	assert_eq!(result.vector[0], 1.0);
	assert_eq!(result.tokens, Some(12));
}

#[rstest]
#[case::wrong_model(json!({"model":"other/model","data":[{"index":0,"embedding":[1.0,0.0]}]}))]
#[case::wrong_dimensions(json!({"model":"google/gemini-embedding-2","data":[{"index":0,"embedding":[1.0,0.0]}]}))]
#[case::wrong_index(json!({"model":"google/gemini-embedding-2","data":[{"index":1,"embedding":[1.0]}]}))]
#[case::zero_vector(json!({"model":"google/gemini-embedding-2","data":[{"index":0,"embedding":[0.0]}]}))]
#[case::provider_error(json!({"error":{"message":"private upstream details"}}))]
#[tokio::test]
async fn openrouter_withholds_output_that_violates_the_approved_contract(#[case] body: Value) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/api/v1", listener.local_addr().unwrap());
	let app = Router::new().route(
		"/api/v1/embeddings",
		post(move || {
			let body = body.clone();
			async move { Json(body) }
		}),
	);
	let _server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	}));
	let mut config = configuration(endpoint);
	config.dimensions = 1;
	assert!(matches!(
		embed(
			&aidash_application::provider_access::EnvironmentAccess {
				credentials: Arc::new(FixtureCredentials)
			},
			&Context::default(),
			&client().unwrap(),
			&config,
			"query"
		)
		.await,
		Err(Error::RemoteSemantic(
			aidash_domain::semantic::Failure::ProviderContract
		))
	));
}
