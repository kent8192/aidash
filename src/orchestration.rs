//! Namespaced, read-only deployment observations for the operator dashboard.
use crate::{Error, Result, federation::Federation};
use axum::{Json, middleware};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, time::Duration};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

#[derive(Serialize, ToSchema)]
pub struct DeploymentStatus {
	pub enabled: bool,
	pub namespace: Option<String>,
	pub release: Option<String>,
	pub deployments: Vec<Deployment>,
	pub pods: Vec<Pod>,
	pub events: Vec<DeploymentEvent>,
}
#[derive(Serialize, ToSchema)]
#[schema(as = KubernetesDeployment)]
pub struct Deployment {
	pub name: String,
	pub role: String,
	pub desired: u64,
	pub ready: u64,
	pub updated: u64,
	pub available: u64,
	pub observed: bool,
	pub conditions: Vec<Condition>,
}
#[derive(Serialize, ToSchema)]
#[schema(as = KubernetesCondition)]
pub struct Condition {
	pub kind: String,
	pub status: String,
	pub reason: String,
	pub message: String,
}
#[derive(Serialize, ToSchema)]
#[schema(as = KubernetesPod)]
pub struct Pod {
	pub name: String,
	pub role: String,
	pub phase: String,
	pub ready: bool,
	pub terminating: bool,
	pub restarts: u64,
	pub conditions: Vec<Condition>,
}
#[derive(Serialize, ToSchema)]
pub struct DeploymentEvent {
	pub object: String,
	pub kind: String,
	pub reason: String,
	pub message: String,
	pub count: u64,
	pub time: Option<String>,
}

pub fn routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new()
		.routes(routes!(status))
		.route_layer(middleware::from_fn(crate::api::operator_only))
}

#[utoipa::path(get,path="/deployment",operation_id="deployment_status",responses((status=200,body=DeploymentStatus),(status=503,description="Kubernetes observations unavailable")),security(("bearer_auth"=[])))]
async fn status() -> Result<Json<DeploymentStatus>> {
	static CLIENT: tokio::sync::OnceCell<Option<Kubernetes>> = tokio::sync::OnceCell::const_new();
	let client = CLIENT.get_or_try_init(Kubernetes::from_env).await?;
	let Some(client) = client else {
		return Ok(Json(DeploymentStatus {
			enabled: false,
			namespace: None,
			release: None,
			deployments: vec![],
			pods: vec![],
			events: vec![],
		}));
	};
	// Projected service-account tokens rotate. Read the current file for each
	// observation; never expose it or persist it in an API response.
	let token = tokio::fs::read_to_string(&client.token_file)
		.await
		.map_err(|_| Error::OrchestrationUnavailable)?;
	if token.len() > 32768 || token.trim().is_empty() {
		return Err(Error::OrchestrationUnavailable);
	}
	let result = tokio::time::timeout(Duration::from_secs(12), client.observe(token.trim())).await;
	result
		.map_err(|_| Error::OrchestrationUnavailable)?
		.map(Json)
		.map_err(|_| Error::OrchestrationUnavailable)
}

struct Kubernetes {
	client: reqwest::Client,
	endpoint: String,
	namespace: String,
	release: String,
	token_file: String,
}
fn label(value: &str) -> bool {
	!value.is_empty()
		&& value.len() <= 63
		&& value
			.bytes()
			.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
		&& !value.starts_with('-')
		&& !value.ends_with('-')
}
impl Kubernetes {
	async fn from_env() -> Result<Option<Self>> {
		let Ok(namespace) = std::env::var("AIDASH_KUBERNETES_NAMESPACE") else {
			return Ok(None);
		};
		let release = std::env::var("AIDASH_KUBERNETES_RELEASE").unwrap_or_default();
		if !label(&namespace) || !label(&release) {
			return Err(Error::Invalid(
				"invalid Kubernetes namespace or release".into(),
			));
		}
		let endpoint = std::env::var("AIDASH_KUBERNETES_API")
			.unwrap_or_else(|_| "https://kubernetes.default.svc".into());
		crate::config::validate_endpoint(&endpoint)?;
		let mut builder = reqwest::Client::builder()
			.timeout(Duration::from_secs(5))
			.connect_timeout(Duration::from_secs(2))
			.redirect(reqwest::redirect::Policy::none());
		if endpoint.starts_with("https://") {
			let ca = std::env::var("AIDASH_KUBERNETES_CA_FILE")
				.unwrap_or_else(|_| "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt".into());
			let ca = tokio::fs::read(ca).await?;
			builder = builder.add_root_certificate(reqwest::Certificate::from_pem(&ca)?);
		}
		Ok(Some(Self {
			client: builder.build()?,
			endpoint: endpoint.trim_end_matches('/').into(),
			namespace,
			release,
			token_file: std::env::var("AIDASH_KUBERNETES_TOKEN_FILE")
				.unwrap_or_else(|_| "/var/run/secrets/kubernetes.io/serviceaccount/token".into()),
		}))
	}
	async fn list(
		&self,
		prefix: &str,
		resource: &str,
		token: &str,
		selected: bool,
	) -> Result<Vec<Value>> {
		#[derive(Deserialize)]
		struct List {
			items: Vec<Value>,
			metadata: Metadata,
		}
		#[derive(Deserialize)]
		struct Metadata {
			#[serde(rename = "continue", default)]
			continuation: String,
		}
		let mut items = vec![];
		let mut continuation = String::new();
		for _ in 0..20 {
			let mut url = reqwest::Url::parse(&format!(
				"{}/{prefix}/namespaces/{}/{resource}",
				self.endpoint, self.namespace
			))
			.map_err(|_| Error::OrchestrationUnavailable)?;
			url.query_pairs_mut()
				.append_pair("limit", "100")
				.append_pair("continue", &continuation);
			if selected {
				url.query_pairs_mut().append_pair(
					"labelSelector",
					&format!(
						"app.kubernetes.io/name=aidash,app.kubernetes.io/instance={}",
						self.release
					),
				);
			}
			let response = self
				.client
				.get(url)
				.bearer_auth(token)
				.send()
				.await?
				.error_for_status()?;
			let page: List = crate::response::json(response, 2 * 1024 * 1024).await?;
			items.extend(page.items);
			if items.len() > 2000 {
				break;
			}
			if page.metadata.continuation.is_empty() {
				return Ok(items);
			}
			continuation = page.metadata.continuation;
		}
		Err(Error::OrchestrationUnavailable)
	}
	async fn observe(&self, token: &str) -> Result<DeploymentStatus> {
		let (deployments, pods, events) = tokio::try_join!(
			self.list("apis/apps/v1", "deployments", token, true),
			self.list("api/v1", "pods", token, true),
			self.list("api/v1", "events", token, false),
		)?;
		Ok(snapshot(
			&self.namespace,
			&self.release,
			deployments,
			pods,
			events,
		))
	}
}
fn text(value: &Value, name: &str) -> String {
	value[name].as_str().unwrap_or_default().into()
}
fn conditions(value: &Value) -> Vec<Condition> {
	value
		.as_array()
		.into_iter()
		.flatten()
		.map(|v| Condition {
			kind: text(v, "type"),
			status: text(v, "status"),
			reason: text(v, "reason"),
			message: text(v, "message"),
		})
		.collect()
}
fn selected(value: &Value, namespace: &str, release: &str) -> bool {
	value["metadata"]["namespace"] == namespace
		&& value["metadata"]["labels"]["app.kubernetes.io/name"] == "aidash"
		&& value["metadata"]["labels"]["app.kubernetes.io/instance"] == release
}
fn snapshot(
	namespace: &str,
	release: &str,
	deployments: Vec<Value>,
	pods: Vec<Value>,
	events: Vec<Value>,
) -> DeploymentStatus {
	let deployments: Vec<_> = deployments
		.into_iter()
		.filter(|v| selected(v, namespace, release))
		.collect();
	let pods: Vec<_> = pods
		.into_iter()
		.filter(|v| selected(v, namespace, release))
		.collect();
	let ids: BTreeSet<_> = deployments
		.iter()
		.chain(&pods)
		.filter_map(|v| v["metadata"]["uid"].as_str())
		.collect();
	let events = events
		.into_iter()
		.filter(|v| {
			v["metadata"]["namespace"] == namespace
				&& v["involvedObject"]["uid"]
					.as_str()
					.is_some_and(|id| ids.contains(id))
		})
		.map(|v| DeploymentEvent {
			object: text(&v["involvedObject"], "name"),
			kind: text(&v, "type"),
			reason: text(&v, "reason"),
			message: text(&v, "message"),
			count: v["count"].as_u64().unwrap_or(1),
			time: v["lastTimestamp"]
				.as_str()
				.or(v["eventTime"].as_str())
				.map(str::to_owned),
		})
		.collect();
	let deployments = deployments
		.iter()
		.map(|v| Deployment {
			name: text(&v["metadata"], "name"),
			role: text(&v["metadata"]["labels"], "app.kubernetes.io/component"),
			desired: v["spec"]["replicas"].as_u64().unwrap_or(1),
			ready: v["status"]["readyReplicas"].as_u64().unwrap_or(0),
			updated: v["status"]["updatedReplicas"].as_u64().unwrap_or(0),
			available: v["status"]["availableReplicas"].as_u64().unwrap_or(0),
			observed: v["status"]["observedGeneration"]
				.as_u64()
				.zip(v["metadata"]["generation"].as_u64())
				.is_some_and(|(o, g)| o >= g),
			conditions: conditions(&v["status"]["conditions"]),
		})
		.collect();
	let pods = pods
		.iter()
		.map(|v| {
			let mut conditions = conditions(&v["status"]["conditions"]);
			let containers = v["status"]["containerStatuses"]
				.as_array()
				.into_iter()
				.flatten();
			let mut restarts = 0;
			for c in containers {
				restarts += c["restartCount"].as_u64().unwrap_or(0);
				for state in ["waiting", "terminated"] {
					if c["state"][state].is_object() {
						conditions.push(Condition {
							kind: text(c, "name"),
							status: state.into(),
							reason: text(&c["state"][state], "reason"),
							message: text(&c["state"][state], "message"),
						});
					}
				}
			}
			Pod {
				name: text(&v["metadata"], "name"),
				role: text(&v["metadata"]["labels"], "app.kubernetes.io/component"),
				phase: text(&v["status"], "phase"),
				ready: conditions
					.iter()
					.any(|c| c.kind == "Ready" && c.status == "True"),
				terminating: !v["metadata"]["deletionTimestamp"].is_null(),
				restarts,
				conditions,
			}
		})
		.collect();
	DeploymentStatus {
		enabled: true,
		namespace: Some(namespace.into()),
		release: Some(release.into()),
		deployments,
		pods,
		events,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use axum::{
		Router,
		extract::{Query, State},
		http::HeaderMap,
		routing::get,
	};
	use serde_json::json;
	use std::sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	};

	#[tokio::test]
	async fn paginated_observations_filter_other_releases_and_reject_partial_failure() {
		let calls = Arc::new(AtomicUsize::new(0));
		async fn handler(
			State(calls): State<Arc<AtomicUsize>>,
			headers: HeaderMap,
			Query(query): Query<std::collections::BTreeMap<String, String>>,
		) -> Json<Value> {
			assert_eq!(headers["authorization"], "Bearer fixture-token");
			assert!(query["labelSelector"].ends_with("=release-a"));
			calls.fetch_add(1, Ordering::SeqCst);
			if query["continue"].is_empty() {
				Json(json!({"items":[{"first":true}],"metadata":{"continue":"next+page/value"}}))
			} else {
				assert_eq!(query["continue"], "next+page/value");
				Json(json!({"items":[{"second":true}],"metadata":{}}))
			}
		}
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let endpoint = format!("http://{}", listener.local_addr().unwrap());
		let app = Router::new()
			.route("/api/v1/namespaces/tenant-a/pods", get(handler))
			.with_state(calls.clone());
		let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
		let client = Kubernetes {
			client: reqwest::Client::new(),
			endpoint,
			namespace: "tenant-a".into(),
			release: "release-a".into(),
			token_file: String::new(),
		};
		assert_eq!(
			client
				.list("api/v1", "pods", "fixture-token", true)
				.await
				.unwrap()
				.len(),
			2
		);
		assert_eq!(calls.load(Ordering::SeqCst), 2);
		assert!(client.observe("fixture-token").await.is_err());
		server.abort();

		let metadata = |name: &str, release: &str| json!({"name":name,"uid":name,"namespace":"tenant-a","generation":2,"labels":{"app.kubernetes.io/name":"aidash","app.kubernetes.io/instance":release,"app.kubernetes.io/component":"worker"}});
		let own = json!({"metadata":metadata("own","release-a"),"spec":{"replicas":3},"status":{"readyReplicas":1,"observedGeneration":1}});
		let other = json!({"metadata":metadata("other","release-b")});
		let pod = json!({"metadata":metadata("pod","release-a"),"status":{"phase":"Pending","containerStatuses":[{"name":"aidash","restartCount":2,"state":{"waiting":{"reason":"CrashLoopBackOff","message":"restart pending"}}}]}});
		let events = vec![
			json!({"metadata":{"namespace":"tenant-a"},"involvedObject":{"uid":"pod","name":"pod"},"reason":"BackOff"}),
			json!({"metadata":{"namespace":"tenant-a"},"involvedObject":{"uid":"other"},"message":"must not leak"}),
		];
		let status = snapshot(
			"tenant-a",
			"release-a",
			vec![own, other.clone()],
			vec![pod, other],
			events,
		);
		assert_eq!(status.deployments.len(), 1);
		assert_eq!(status.deployments[0].desired, 3);
		assert!(!status.deployments[0].observed);
		assert_eq!(status.pods.len(), 1);
		assert_eq!(status.pods[0].restarts, 2);
		assert_eq!(status.pods[0].conditions[0].reason, "CrashLoopBackOff");
		assert_eq!(status.events.len(), 1);
		assert!(!label("release,other=value"));
	}
}
