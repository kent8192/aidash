use super::*;
use axum::{
	Json, Router,
	extract::{Query, State},
	http::HeaderMap,
	routing::get,
};
use rstest::rstest;
use serde_json::json;
use std::{
	collections::BTreeMap,
	sync::{
		Arc, Mutex,
		atomic::{AtomicUsize, Ordering},
	},
};

struct TestServer(tokio::task::JoinHandle<()>);
impl Drop for TestServer {
	fn drop(&mut self) {
		self.0.abort();
	}
}

async fn serve(router: Router) -> (String, TestServer) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
	(endpoint, TestServer(task))
}

fn settings(endpoint: String, token_file: String) -> Settings {
	Settings {
		endpoint,
		token_file,
		namespace: "tenant-a".into(),
		release: "release-a".into(),
		ca_file: String::new(),
	}
}

#[rstest]
#[tokio::test]
async fn pagination_preserves_opaque_continuations_and_rejects_partial_observations() {
	// Arrange
	let calls = Arc::new(AtomicUsize::new(0));
	async fn handler(
		State(calls): State<Arc<AtomicUsize>>,
		headers: HeaderMap,
		Query(query): Query<BTreeMap<String, String>>,
	) -> Json<Value> {
		assert_eq!(headers["authorization"], "Bearer fixture-token");
		assert!(query["labelSelector"].ends_with("=release-a"));
		assert_eq!(query["limit"], "100");
		calls.fetch_add(1, Ordering::SeqCst);
		if query["continue"].is_empty() {
			Json(json!({"items":[{"first":true}],"metadata":{"continue":"next+page/value"}}))
		} else {
			assert_eq!(query["continue"], "next+page/value");
			Json(json!({"items":[{"second":true}],"metadata":{}}))
		}
	}
	let (endpoint, _server) = serve(
		Router::new()
			.route("/api/v1/namespaces/tenant-a/pods", get(handler))
			.with_state(calls.clone()),
	)
	.await;
	let client = Kubernetes::new(settings(endpoint, String::new()));
	// Act
	let pages = client
		.list("api/v1", "pods", "fixture-token", true)
		.await
		.unwrap();
	// Assert
	assert_eq!(pages.len(), 2);
	assert_eq!(calls.load(Ordering::SeqCst), 2);
	assert!(client.observe_with_token("fixture-token").await.is_err());
}

#[rstest]
#[tokio::test]
async fn projected_tokens_are_resolved_again_after_rotation() {
	// Arrange
	let headers_seen = Arc::new(Mutex::new(Vec::new()));
	async fn handler(
		State(seen): State<Arc<Mutex<Vec<String>>>>,
		headers: HeaderMap,
	) -> Json<Value> {
		seen.lock()
			.unwrap()
			.push(headers["authorization"].to_str().unwrap().into());
		Json(json!({"items":[],"metadata":{}}))
	}
	let router = Router::new()
		.route(
			"/apis/apps/v1/namespaces/tenant-a/deployments",
			get(handler),
		)
		.route("/api/v1/namespaces/tenant-a/pods", get(handler))
		.route("/api/v1/namespaces/tenant-a/events", get(handler))
		.with_state(headers_seen.clone());
	let (endpoint, _server) = serve(router).await;
	let directory = tempfile::tempdir().unwrap();
	let token_file = directory.path().join("token");
	let client = Kubernetes::new(settings(endpoint, token_file.to_string_lossy().into()));
	// Act
	tokio::fs::write(&token_file, "first-token\n")
		.await
		.unwrap();
	client.observe().await.unwrap();
	tokio::fs::write(&token_file, "second-token\n")
		.await
		.unwrap();
	client.observe().await.unwrap();
	// Assert
	let seen = headers_seen.lock().unwrap();
	assert_eq!(seen.len(), 6);
	assert!(
		seen[..3]
			.iter()
			.all(|header| header == "Bearer first-token")
	);
	assert!(
		seen[3..]
			.iter()
			.all(|header| header == "Bearer second-token")
	);
}

#[rstest]
fn protocol_decoding_preserves_replica_conditions_and_restart_details() {
	// Arrange
	let metadata = |name: &str, release: &str| json!({"name":name,"uid":name,"namespace":"tenant-a","generation":2,"labels":{"app.kubernetes.io/name":"aidash","app.kubernetes.io/instance":release,"app.kubernetes.io/component":"worker"}});
	let own = json!({"metadata":metadata("own","release-a"),"spec":{"replicas":3},"status":{"readyReplicas":1,"observedGeneration":1}});
	let other = json!({"metadata":metadata("other","release-b")});
	let pod = json!({"metadata":metadata("pod","release-a"),"status":{"phase":"Pending","containerStatuses":[{"name":"aidash","restartCount":2,"state":{"waiting":{"reason":"CrashLoopBackOff","message":"restart pending"}}}]}});
	let events = vec![
		json!({"metadata":{"namespace":"tenant-a"},"involvedObject":{"uid":"pod","name":"pod"},"reason":"BackOff"}),
		json!({"metadata":{"namespace":"tenant-a"},"involvedObject":{"uid":"other"},"message":"must not leak"}),
	];
	// Act
	let observations = inventory(vec![own, other.clone()], vec![pod, other], events);
	let status = aidash_domain::deployment::DeploymentStatus::snapshot(
		"tenant-a",
		"release-a",
		observations,
	);
	// Assert
	assert_eq!(status.deployments.len(), 1);
	assert_eq!(status.deployments[0].desired, 3);
	assert!(!status.deployments[0].observed);
	assert_eq!(status.pods.len(), 1);
	assert_eq!(status.pods[0].restarts, 2);
	assert_eq!(status.pods[0].conditions[0].reason, "CrashLoopBackOff");
	assert_eq!(status.events.len(), 1);
}
