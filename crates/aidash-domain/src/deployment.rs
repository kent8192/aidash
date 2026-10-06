//! Deployment observations and the release boundary exposed to operators.
use schemars::JsonSchema;
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeploymentStatus {
	pub enabled: bool,
	pub namespace: Option<String>,
	pub release: Option<String>,
	pub deployments: Vec<Deployment>,
	pub pods: Vec<Pod>,
	pub events: Vec<DeploymentEvent>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "KubernetesDeployment")]
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

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "KubernetesCondition")]
pub struct Condition {
	pub kind: String,
	pub status: String,
	pub reason: String,
	pub message: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "KubernetesPod")]
pub struct Pod {
	pub name: String,
	pub role: String,
	pub phase: String,
	pub ready: bool,
	pub terminating: bool,
	pub restarts: u64,
	pub conditions: Vec<Condition>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeploymentEvent {
	pub object: String,
	pub kind: String,
	pub reason: String,
	pub message: String,
	pub count: u64,
	pub time: Option<String>,
}

/// An external observation carries provenance separately from its public value.
#[derive(Debug)]
pub struct ScopedObservation<T> {
	pub namespace: String,
	pub application: String,
	pub release: String,
	pub uid: Option<String>,
	pub value: T,
}

impl<T> ScopedObservation<T> {
	fn selected(&self, namespace: &str, release: &str) -> bool {
		self.namespace == namespace && self.application == "aidash" && self.release == release
	}
}

#[derive(Debug)]
pub struct EventObservation {
	pub namespace: String,
	pub object_uid: Option<String>,
	pub value: DeploymentEvent,
}

#[derive(Debug, Default)]
pub struct DeploymentInventory {
	pub deployments: Vec<ScopedObservation<Deployment>>,
	pub pods: Vec<ScopedObservation<Pod>>,
	pub events: Vec<EventObservation>,
}

pub fn valid_label(value: &str) -> bool {
	!value.is_empty()
		&& value.len() <= 63
		&& value
			.bytes()
			.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
		&& !value.starts_with('-')
		&& !value.ends_with('-')
}

impl DeploymentStatus {
	pub fn disabled() -> Self {
		Self {
			enabled: false,
			namespace: None,
			release: None,
			deployments: vec![],
			pods: vec![],
			events: vec![],
		}
	}

	/// Keep the configured namespace/release and events for those exact object IDs.
	/// The boundary is checked locally even when the external API accepts selectors.
	pub fn snapshot(namespace: &str, release: &str, inventory: DeploymentInventory) -> Self {
		let deployments: Vec<_> = inventory
			.deployments
			.into_iter()
			.filter(|v| v.selected(namespace, release))
			.collect();
		let pods: Vec<_> = inventory
			.pods
			.into_iter()
			.filter(|v| v.selected(namespace, release))
			.collect();
		let ids: BTreeSet<_> = deployments
			.iter()
			.filter_map(|v| v.uid.as_deref())
			.chain(pods.iter().filter_map(|v| v.uid.as_deref()))
			.collect();
		let events = inventory
			.events
			.into_iter()
			.filter(|event| {
				event.namespace == namespace
					&& event
						.object_uid
						.as_deref()
						.is_some_and(|uid| ids.contains(uid))
			})
			.map(|event| event.value)
			.collect();
		Self {
			enabled: true,
			namespace: Some(namespace.into()),
			release: Some(release.into()),
			deployments: deployments.into_iter().map(|v| v.value).collect(),
			pods: pods.into_iter().map(|v| v.value).collect(),
			events,
		}
	}
}

#[cfg(test)]
mod tests;
