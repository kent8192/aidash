use super::*;
use aidash_application::ports::capabilities::execution_pod::{
	CommandRequest, ExecutionPodError, ExecutionPodErrorCode, ExecutionPodRuntime, RunStopReason,
};
use axum::{
	Json, Router,
	body::Body,
	extract::Path,
	http::{HeaderMap, StatusCode},
	response::{IntoResponse, Response},
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
        .route("/v2/commands/submit", post(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer before-rotation");
            assert_eq!(body["scratch_space_id"], "space-a");
            assert_eq!(body["run_id"], "run-a");
            assert_eq!(body["background"], true);
            Json(json!({"operation_id":body["operation_id"],"scratch_space_id":"space-a","run_id":"run-a","status":"queued","background":true}))
        }))
        .route("/v2/pods/stop", post(|| async { (StatusCode::CONFLICT, Json(json!({"code":"termination_unconfirmed","message":"guard unavailable"}))) }))
        .route("/v2/pods/delete", post(|| async { Json(json!({"deleted":false})) }))
        .route("/v2/runs/cancel", post(|Json(body): Json<Value>| async move {
            assert_eq!(body, json!({"scratch_space_id":"space-a","run_id":"run-a"}));
            Json(json!({"cancelled_waiting":true}))
        }))
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
		)
		.route(
			"/oversize",
			get(|| async { Response::new(Body::from(vec![b' '; MAX_RUNNER_RESPONSE_BYTES + 1])) }),
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
async fn oversized_runner_response_is_rejected_before_json_decoding() {
	let (_server, client, _keys) = fixture().await;
	assert!(matches!(
		client.request("GET", "/oversize", None).await,
		Err(Error::External(message)) if message.contains("response exceeds 33554432 bytes")
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
		matches!(client.request("GET","/v1/health",None).await,Err(Error::Invalid(message)) if message=="runner transport requires TLS, loopback or a cluster Service")
	);
}

#[tokio::test]
async fn the_configured_cluster_runner_requires_a_credential_before_connecting() {
	let client = RunnerHttp {
		endpoint: "http://app-execution-runner.application.svc.cluster.local:8949".into(),
		credential_env: "RUNNER_KEY".into(),
		credentials: Arc::new(Keys(RwLock::new(None))),
	};
	assert!(matches!(client.request("GET", "/v1/health", None).await,
		Err(Error::Conflict(message)) if message.starts_with("RUNTIME_UNAVAILABLE:")));
}

#[tokio::test]
async fn execution_pod_requests_preserve_identity_and_remote_failure_codes() {
	let (_server, client, _) = fixture().await;
	let request = CommandRequest {
		operation_id: "op".into(),
		scratch_space_id: "space-a".into(),
		run_id: "run-a".into(),
		code: "sleep 10".into(),
		expected_session_id: None,
		cwd: None,
		timeout_seconds: None,
		background: true,
		notify_on_exit: false,
		proxy_credential: None,
	};
	let accepted = client.execute(&request).await.unwrap();
	assert_eq!(accepted.status, "queued");
	assert_eq!(accepted.run_id.as_deref(), Some("run-a"));
	client.cancel_waiting("space-a", "run-a").await.unwrap();
	assert!(
		matches!(client.stop("space-a").await, Err(ExecutionPodError::Remote { code, message }) if code == ExecutionPodErrorCode::TerminationUnconfirmed && message == "guard unavailable")
	);
	assert!(client.delete("space-a").await.is_err());
}

#[tokio::test]
async fn execution_pod_v2_transports_process_credentials_and_file_bytes() {
	use aidash_application::ports::capabilities::execution_pod::{
		EgressCredential, FileWriteRequest, PythonStartRequest,
	};
	let app = Router::new()
		.route("/v2/python/start", post(|Json(body): Json<Value>| async move {
			assert_eq!(body, json!({"scratch_space_id":"s","run_id":"r","proxy_credential":"secret"}));
			Json(json!({"operation_id":"p","scratch_space_id":"s","run_id":"r","socket_path":"/tmp/p"}))
		}))
		.route("/v2/files/write", post(|Json(body): Json<Value>| async move {
			assert_eq!(body, json!({"scratch_space_id":"s","run_id":"r","path":"x","data":"AP8=","offset":0,"final":true,"mode":null}));
			Json(json!({"size":2}))
		}))
		.route("/v2/runs/stop", post(|Json(body): Json<Value>| async move {
			assert_eq!(body, json!({"scratch_space_id":"s","run_id":"r","reason":"run_authority_revoked"}));
			Json(json!({"stopped":["p","left"]}))
		}));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/", listener.local_addr().unwrap());
	let _server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap()
	}));
	let client = RunnerHttp {
		endpoint,
		credential_env: "RUNNER_KEY".into(),
		credentials: Arc::new(Keys(RwLock::new(Some("before-rotation".into())))),
	};
	let session = client
		.python_start(&PythonStartRequest {
			scratch_space_id: "s".into(),
			run_id: "r".into(),
			proxy_credential: Some(EgressCredential("secret".into())),
		})
		.await
		.unwrap();
	assert_eq!(session.operation_id, "p");
	let written = client
		.file_write(&FileWriteRequest {
			scratch_space_id: "s".into(),
			run_id: "r".into(),
			path: "x".into(),
			data: "AP8=".into(),
			offset: 0,
			final_chunk: true,
			mode: None,
		})
		.await
		.unwrap();
	assert_eq!(written.size, 2);
	assert_eq!(
		client
			.stop_run("s", "r", RunStopReason::RunAuthorityRevoked)
			.await
			.unwrap(),
		vec!["p", "left"]
	);
}

#[tokio::test]
async fn execution_pod_routes_exchange_their_v2_wire_contracts() {
	use aidash_application::ports::capabilities::execution_pod::*;
	use axum::extract::OriginalUri;
	let app = Router::new().route("/v2/{*tail}", post(|OriginalUri(uri): OriginalUri, Json(body): Json<Value>| async move {
		let response = match uri.path() {
			"/v2/commands/refuse" => { assert_eq!(body, json!({"operation_id":"op","scratch_space_id":"s","run_id":"r"})); json!({"refused":false,"status":"running"}) },
			"/v2/background/log" => { assert_eq!(body, json!({"operation_id":"op","stream":"stderr","offset":null,"limit":4})); json!({"data":"AP8=","next_offset":2,"eof":true,"size":2}) },
			"/v2/files/read" => { assert_eq!(body, json!({"scratch_space_id":"s","path":"a","offset":1,"limit":4})); json!({"data":"/w==","size":2,"next_offset":2,"eof":true}) },
			"/v2/files/search" => { assert_eq!(body, json!({"scratch_space_id":"s","pattern":"needle","path":null,"glob":null,"regex":false,"max_results":5})); json!({"matches":[{"path":"a","line":2,"text":"needle"}],"truncated":false}) },
			"/v2/files/patch" => { assert_eq!(body, json!({"scratch_space_id":"s","run_id":"r","patch":"patch","claim_id":uuid::Uuid::nil(),"apply":true})); json!({"changed":["a"]}) },
			"/v2/files/manifest" => { assert_eq!(body, json!({"scratch_space_id":"s","root":".","ignore_files":[".gitignore"],"exclude":[],"include":[],"max_files":4})); json!({"files":[{"path":"a","size":2,"sha256":"digest"}],"skipped_symlinks":["b"],"truncated":false}) },
			"/v2/previews/start" => { assert_eq!(body, json!({"operation_id":"op","scratch_space_id":"s","run_id":"r","code":"serve","port":8080,"proxy_credential":"secret"})); json!({"operation_id":"op","address":"10.0.0.1:8080"}) },
			// A stop answers as soon as it signals; the end shows in the result.
			"/v2/previews/stop" | "/v2/mcp/stop" => { assert_eq!(body, json!({"operation_id":"op"})); json!({"operation_id":"op","status":"stopping"}) },
			"/v2/commands/result" => { assert_eq!(body, json!({"operation_id":"op"})); json!({"operation_id":"op","status":"stopped"}) },
			"/v2/mcp/start" => { assert_eq!(body, json!({"operation_id":"op","scratch_space_id":"s","run_id":"r","install":"","command":["server"],"env":{"MODE":"test"},"plugin_data":"data","proxy_credential":"secret"})); json!({"operation_id":"op"}) },
			"/v2/mcp/call" => { assert_eq!(body, json!({"operation_id":"op","message":{"jsonrpc":"2.0","id":1},"timeout_seconds":3})); json!({"message":{"jsonrpc":"2.0","id":1,"result":{}}}) },
			other => panic!("unexpected path {other}"),
		}; Json(response)
	}));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/", listener.local_addr().unwrap());
	let _server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap()
	}));
	let client = RunnerHttp {
		endpoint,
		credential_env: "RUNNER_KEY".into(),
		credentials: Arc::new(Keys(RwLock::new(Some("before-rotation".into())))),
	};
	let log: LogRequest = serde_json::from_value(
		json!({"operation_id":"op","stream":"stderr","offset":null,"limit":4}),
	)
	.unwrap();
	assert_eq!(client.background_log(&log).await.unwrap().data, "AP8=");
	assert_eq!(
		client.refuse_command("op", "s", "r").await.unwrap(),
		CommandRefusal {
			refused: false,
			status: Some("running".into()),
		}
	);
	let read: FileReadRequest =
		serde_json::from_value(json!({"scratch_space_id":"s","path":"a","offset":1,"limit":4}))
			.unwrap();
	assert_eq!(client.file_read(&read).await.unwrap().data, "/w==");
	let search: FileSearchRequest = serde_json::from_value(json!({"scratch_space_id":"s","pattern":"needle","path":null,"glob":null,"regex":false,"max_results":5})).unwrap();
	assert_eq!(
		client.file_search(&search).await.unwrap().matches[0].line,
		2
	);
	let patch: FilePatchRequest =
		serde_json::from_value(json!({"scratch_space_id":"s","run_id":"r","patch":"patch","claim_id":uuid::Uuid::nil(),"apply":true}))
			.unwrap();
	assert_eq!(client.file_patch(&patch).await.unwrap().changed, vec!["a"]);
	let manifest: FileManifestRequest = serde_json::from_value(json!({"scratch_space_id":"s","root":".","ignore_files":[".gitignore"],"exclude":[],"include":[],"max_files":4})).unwrap();
	assert_eq!(
		client
			.file_manifest(&manifest)
			.await
			.unwrap()
			.skipped_symlinks,
		vec!["b"]
	);
	let preview: PreviewStartRequest = serde_json::from_value(json!({"operation_id":"op","scratch_space_id":"s","run_id":"r","code":"serve","cwd":null,"port":8080,"proxy_credential":"secret"})).unwrap();
	assert_eq!(
		client.preview_start(&preview).await.unwrap().address,
		"10.0.0.1:8080"
	);
	client.preview_stop("op").await.unwrap();
	let mcp: McpStartRequest = serde_json::from_value(json!({"operation_id":"op","scratch_space_id":"s","run_id":"r","install":"","command":["server"],"env":{"MODE":"test"},"plugin_data":"data","proxy_credential":"secret"})).unwrap();
	assert_eq!(client.mcp_start(&mcp).await.unwrap().operation_id, "op");
	let call: McpCallRequest = serde_json::from_value(
		json!({"operation_id":"op","message":{"jsonrpc":"2.0","id":1},"timeout_seconds":3}),
	)
	.unwrap();
	assert_eq!(client.mcp_call(&call).await.unwrap().message["id"], 1);
	client.mcp_stop("op").await.unwrap();
}
