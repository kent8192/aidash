use super::*;
use axum::{Json, Router, http::HeaderMap, routing::post};
use rstest::rstest;

#[rstest]
#[case(404, json!({"error":{"code":"model_not_found"}}), true)]
#[case(400, json!({"error":{"code":"model_not_found"}}), true)]
#[case(404, json!({"error":{"message":"Model does not exist"}}), true)]
#[case(404, json!({"error":{"message":"Endpoint not found"}}), false)]
#[case(500, json!({"error":{"code":"model_not_found"}}), false)]
fn missing_model_classification_is_distinct_from_outages_and_missing_endpoints(
	#[case] status: u16,
	#[case] body: Value,
	#[case] expected: bool,
) {
	assert_eq!(embedding_model_missing(status, Some(&body), None), expected);
	assert!(embedding_model_missing(404, None, Some(b"model not found")));
	assert!(!embedding_model_missing(
		404,
		None,
		Some(b"endpoint not found")
	));
}

/// A Tenant Provider Credential stand-in: the bearer comes from provider access.
struct FixtureAccess;
#[async_trait::async_trait]
impl aidash_application::provider_access::ProviderAccess for FixtureAccess {
	async fn resolve(
		&self,
		_: &Context,
		endpoint: &str,
		_: &aidash_application::provider_access::Source,
	) -> Result<aidash_application::provider_access::Access> {
		Ok(aidash_application::provider_access::Access {
			endpoint: endpoint.into(),
			bearer: "fixture-key".to_owned().into(),
			keyless: None,
		})
	}
	async fn check(
		&self,
		_: &Context,
		_: &str,
		_: &aidash_application::provider_access::Source,
	) -> Result<()> {
		unreachable!("embedding never preflights a credential")
	}
}

/// Keyless access that permits this fixture's loopback origin (ADR 0036).
fn permitting(endpoint: &str) -> aidash_application::provider_access::KeylessAccess {
	let origin = reqwest::Url::parse(endpoint)
		.unwrap()
		.origin()
		.ascii_serialization();
	aidash_application::provider_access::KeylessAccess::permitting(
		aidash_domain::keyless::PermittedOrigins::parse([origin]).unwrap(),
	)
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
		provider_credential: None,
		model: "google/gemini-embedding-2".into(),
		model_version: "1.0.0".into(),
		dimensions: 3072,
		inputs: [aidash_domain::semantic::EmbeddingInputKind::Text]
			.into_iter()
			.collect(),
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
		&FixtureAccess,
		&Context::default(),
		&clients().unwrap(),
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
			&permitting(&config.endpoint),
			&Context::default(),
			&clients().unwrap(),
			&config,
			"query"
		)
		.await,
		Err(Error::RemoteSemantic(
			aidash_domain::semantic::Failure::ProviderContract
		))
	));
}

/// How an embedding reply's body arrives.
#[derive(Clone, Copy)]
enum Body {
	/// Breaks off mid-transfer.
	Interrupted,
	/// Completes, but is not the declared JSON.
	Malformed,
	/// Completes, but streams past the 1 MiB response bound.
	Oversized,
	/// Declares a `Content-Length` past the 1 MiB response bound.
	DeclaredOversized,
}

/// A reply whose body breaks off mid-transfer is a transport outage that a Run
/// retries, not the non-retried contract violation of a malformed or
/// oversized body.
#[rstest]
#[case::interrupted_body(Body::Interrupted)]
#[case::malformed_body(Body::Malformed)]
#[case::oversized_body(Body::Oversized)]
#[case::declared_oversized_body(Body::DeclaredOversized)]
#[tokio::test]
async fn interrupted_response_body_is_an_outage_and_malformed_json_a_contract_violation(
	#[case] body: Body,
) {
	// Arrange
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/api/v1", listener.local_addr().unwrap());
	let app = Router::new().route(
		"/api/v1/embeddings",
		post(move || async move {
			let chunks: Vec<std::result::Result<String, std::io::Error>> = match body {
				Body::Interrupted => vec![
					Ok(r#"{"model":"#.into()),
					Err(std::io::Error::other("connection reset")),
				],
				Body::Malformed => vec![Ok(r#"{"model":"#.into())],
				// Chunked, so only the streamed length crosses the bound.
				Body::Oversized => vec![Ok(" ".repeat(1_048_576)), Ok("{}".into())],
				// A full body, so its length is declared before any chunk.
				Body::DeclaredOversized => {
					return axum::body::Body::from(" ".repeat(1_048_577));
				}
			};
			axum::body::Body::from_stream(futures_util::stream::iter(chunks))
		}),
	);
	let _server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	}));
	// Act
	let result = embed(
		&permitting(&endpoint),
		&Context::default(),
		&clients().unwrap(),
		&configuration(endpoint),
		"query",
	)
	.await;
	// Assert
	if matches!(body, Body::Interrupted) {
		assert!(matches!(result, Err(Error::External(_))), "{result:?}");
	} else {
		assert!(
			matches!(
				result,
				Err(Error::RemoteSemantic(
					aidash_domain::semantic::Failure::ProviderContract
				))
			),
			"{result:?}"
		);
	}
}

#[tokio::test]
async fn image_embedding_sends_validated_image_as_openrouter_content() {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/api/v1", listener.local_addr().unwrap());
	let app = Router::new().route("/api/v1/embeddings", post(|Json(body): Json<Value>| async move {
        assert_eq!(body["input"], json!([{"content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,iVBORw0KGgo="}}]}]));
        Json(json!({"model":"google/gemini-embedding-2","data":[{"index":0,"embedding":[1.0]}]}))
    }));
	let _server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	}));
	let mut config = configuration(endpoint);
	config.dimensions = 1;
	config
		.inputs
		.insert(aidash_domain::semantic::EmbeddingInputKind::Image);
	let input = aidash_domain::semantic::EmbeddingInput::Image {
		media_type: "image/png",
		bytes: b"\x89PNG\r\n\x1a\n",
	};
	let result = embed_input(
		&FixtureAccess,
		&Context::default(),
		&clients().unwrap(),
		&config,
		input,
	)
	.await
	.unwrap();
	assert_eq!(result.vector, vec![1.0]);
	let invalid = aidash_domain::semantic::EmbeddingInput::Image {
		media_type: "image/jpeg",
		bytes: b"\x89PNG\r\n\x1a\n",
	};
	assert!(
		embed_input(
			&FixtureAccess,
			&Context::default(),
			&clients().unwrap(),
			&config,
			invalid
		)
		.await
		.is_err()
	);
	config
		.inputs
		.remove(&aidash_domain::semantic::EmbeddingInputKind::Image);
	assert!(
		embed_input(
			&FixtureAccess,
			&Context::default(),
			&clients().unwrap(),
			&config,
			input
		)
		.await
		.is_err()
	);
}
