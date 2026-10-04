//! Bounded, read-only Kubernetes adapter with rotating service-account credentials.
use crate::{Error, Result};
use aidash_application::ports::DeploymentObserver;
use aidash_domain::deployment::{
	Condition, Deployment, DeploymentEvent, DeploymentInventory, EventObservation, Pod,
	ScopedObservation,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::OnceCell;

#[derive(Debug, Clone)]
pub struct Settings {
	pub endpoint: String,
	pub namespace: String,
	pub release: String,
	pub ca_file: String,
	pub token_file: String,
}

pub struct Kubernetes {
	settings: Settings,
	client: OnceCell<reqwest::Client>,
}

impl Kubernetes {
	pub fn new(settings: Settings) -> Self {
		Self {
			settings,
			client: OnceCell::new(),
		}
	}

	async fn client(&self) -> Result<&reqwest::Client> {
		self.client
			.get_or_try_init(|| async {
				let mut builder = reqwest::Client::builder()
					.timeout(Duration::from_secs(5))
					.connect_timeout(Duration::from_secs(2))
					.redirect(reqwest::redirect::Policy::none());
				if self.settings.endpoint.starts_with("https://") {
					let ca = tokio::fs::read(&self.settings.ca_file)
						.await
						.map_err(|error| Error::External(error.to_string()))?;
					builder = builder.add_root_certificate(
						reqwest::Certificate::from_pem(&ca).map_err(crate::http_error)?,
					);
				}
				builder.build().map_err(crate::http_error)
			})
			.await
	}

	async fn list(
		&self,
		prefix: &str,
		resource: &str,
		token: &str,
		selected: bool,
	) -> Result<Vec<Value>> {
		let mut items = vec![];
		let mut continuation = String::new();
		for _ in 0..20 {
			let mut url = reqwest::Url::parse(&format!(
				"{}/{prefix}/namespaces/{}/{resource}",
				self.settings.endpoint.trim_end_matches('/'),
				self.settings.namespace
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
						self.settings.release
					),
				);
			}
			let response = self
				.client()
				.await?
				.get(url)
				.bearer_auth(token)
				.send()
				.await
				.map_err(crate::http_error)?
				.error_for_status()
				.map_err(crate::http_error)?;
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

	async fn observe_with_token(&self, token: &str) -> Result<DeploymentInventory> {
		let (deployments, pods, events) = tokio::try_join!(
			self.list("apis/apps/v1", "deployments", token, true),
			self.list("api/v1", "pods", token, true),
			self.list("api/v1", "events", token, false)
		)?;
		Ok(inventory(deployments, pods, events))
	}
}

#[async_trait]
impl DeploymentObserver for Kubernetes {
	async fn observe(&self) -> Result<DeploymentInventory> {
		self.client().await?;
		// Projected tokens rotate: resolve the current file for every observation.
		let token = tokio::fs::read_to_string(&self.settings.token_file)
			.await
			.map_err(|_| Error::OrchestrationUnavailable)?;
		if token.len() > 32768 || token.trim().is_empty() {
			return Err(Error::OrchestrationUnavailable);
		}
		tokio::time::timeout(
			Duration::from_secs(12),
			self.observe_with_token(token.trim()),
		)
		.await
		.map_err(|_| Error::OrchestrationUnavailable)?
		.map_err(|_| Error::OrchestrationUnavailable)
	}
}

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

fn scoped<T>(v: &Value, value: T) -> ScopedObservation<T> {
	ScopedObservation {
		namespace: text(&v["metadata"], "namespace"),
		application: text(&v["metadata"]["labels"], "app.kubernetes.io/name"),
		release: text(&v["metadata"]["labels"], "app.kubernetes.io/instance"),
		uid: v["metadata"]["uid"].as_str().map(str::to_owned),
		value,
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

fn inventory(deployments: Vec<Value>, pods: Vec<Value>, events: Vec<Value>) -> DeploymentInventory {
	let events = events
		.into_iter()
		.map(|v| EventObservation {
			namespace: text(&v["metadata"], "namespace"),
			object_uid: v["involvedObject"]["uid"].as_str().map(str::to_owned),
			value: DeploymentEvent {
				object: text(&v["involvedObject"], "name"),
				kind: text(&v, "type"),
				reason: text(&v, "reason"),
				message: text(&v, "message"),
				count: v["count"].as_u64().unwrap_or(1),
				time: v["lastTimestamp"]
					.as_str()
					.or(v["eventTime"].as_str())
					.map(str::to_owned),
			},
		})
		.collect();
	let deployments = deployments
		.iter()
		.map(|v| {
			scoped(
				v,
				Deployment {
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
				},
			)
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
			scoped(
				v,
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
				},
			)
		})
		.collect();
	DeploymentInventory {
		deployments,
		pods,
		events,
	}
}

#[cfg(test)]
mod tests;
