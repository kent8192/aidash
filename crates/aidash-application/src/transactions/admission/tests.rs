use super::*;
use crate::ports::{
	Credentials, registry::CoreToolCatalog, transactions::admission::OwnedAdmissionScope,
};
use aidash_domain::{
	capabilities::CoreCapabilities,
	provider::ToolSpec,
	transactions::{Isolation, Mutation, Participant},
};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use rstest::{fixture, rstest};
use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
};
use uuid::Uuid;

struct Catalog;
impl Credentials for Catalog {
	fn resolve(&self, name: &str) -> Result<String> {
		Err(Error::NotFound(name.into()))
	}
}
impl CoreToolCatalog for Catalog {
	fn specifications(&self, _config: &CoreCapabilities) -> BTreeMap<String, ToolSpec> {
		BTreeMap::new()
	}
}

#[fixture]
fn validation() -> DefinitionValidation {
	DefinitionValidation::new(Arc::new(Catalog), Arc::new(Catalog))
}

#[fixture]
fn manifest() -> Manifest {
	Manifest {
		id: Uuid::from_u128(1),
		coordinator: "aidash://home".into(),
		isolation: Isolation::Serializable,
		deadline: "2030-01-01T01:00:00Z".parse().unwrap(),
		participants: ["aidash://home", "aidash://peer"]
			.into_iter()
			.map(|node| Participant {
				node_id: node.into(),
				mutations: vec![Mutation::WorkspaceState {
					workspace_id: Uuid::from_u128(2),
					expected_revision: 0,
					state: json!({}),
				}],
			})
			.collect(),
	}
}

#[fixture]
fn origin() -> Origin {
	Origin {
		credential_id: Uuid::from_u128(3),
		tenant: "tenant".into(),
		subject: "owner".into(),
	}
}

fn status(manifest: &Manifest) -> Status {
	Status {
		id: manifest.id,
		digest: manifest.digest().unwrap(),
		manifest: json!(manifest),
		decision: None,
		visible: false,
		complete: false,
		last_error: None,
		created_at: "2030-01-01T00:00:00Z".parse().unwrap(),
	}
}

struct State {
	status: Option<Status>,
	origin: Option<Origin>,
	log: Vec<String>,
	trusted: bool,
	failure: Option<String>,
}

#[derive(Clone)]
struct Adapter(Arc<Mutex<State>>);
impl Adapter {
	fn new() -> Self {
		Self(Arc::new(Mutex::new(State {
			status: None,
			origin: None,
			log: vec![],
			trusted: true,
			failure: None,
		})))
	}
	fn event(&self, event: &str) -> Result<()> {
		let mut state = self.0.lock().unwrap();
		state.log.push(event.into());
		if state.failure.as_deref() == Some(event) {
			return Err(Error::External(event.into()));
		}
		Ok(())
	}
	fn log(&self) -> Vec<String> {
		self.0.lock().unwrap().log.clone()
	}
}

#[async_trait]
impl AdmissionScope for Adapter {
	fn node_id(&self) -> &str {
		"aidash://home"
	}
	fn now(&self) -> DateTime<Utc> {
		"2030-01-01T00:00:00Z".parse().unwrap()
	}
	async fn status(&mut self, _id: Uuid) -> Result<Status> {
		self.event("status")?;
		self.0
			.lock()
			.unwrap()
			.status
			.clone()
			.ok_or_else(|| Error::NotFound("transaction".into()))
	}
	async fn match_origin(&mut self, _id: Uuid, origin: Option<&Origin>) -> Result<()> {
		self.event("match_origin")?;
		if self.0.lock().unwrap().origin.as_ref() != origin {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn resolve_peer(&mut self, _node: &str) -> Result<()> {
		self.event("resolve_peer")
	}
	async fn trusted(&mut self, _node: &str) -> Result<bool> {
		self.event("trusted")?;
		Ok(self.0.lock().unwrap().trusted)
	}
	async fn admit(&mut self, manifest: &Manifest) -> Result<Status> {
		self.event("admit")?;
		Ok(status(manifest))
	}
	async fn bind_origin(&mut self, _id: Uuid, origin: &Origin) -> Result<()> {
		self.event("bind_origin")?;
		self.0.lock().unwrap().origin = Some(origin.clone());
		Ok(())
	}
}

struct Owned {
	adapter: Adapter,
	committed: bool,
}
#[async_trait]
impl OwnedAdmissionScope for Owned {
	fn admission(&mut self) -> Box<dyn AdmissionScope + '_> {
		Box::new(self.adapter.clone())
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		self.adapter.event("commit")?;
		self.committed = true;
		Ok(())
	}
}
impl Drop for Owned {
	fn drop(&mut self) {
		if !self.committed {
			self.adapter.0.lock().unwrap().log.push("rollback".into());
		}
	}
}
#[async_trait]
impl AdmissionRepository for Adapter {
	async fn begin(&self) -> Result<Box<dyn OwnedAdmissionScope>> {
		self.event("begin")?;
		Ok(Box::new(Owned {
			adapter: self.clone(),
			committed: false,
		}))
	}
	async fn fault(&self, _id: Uuid, point: &str) -> Result<()> {
		self.event(point)
	}
	fn wake(&self) {
		self.0.lock().unwrap().log.push("wake".into());
	}
}

#[rstest]
#[case::below_minimum(999, false)]
#[case::minimum(1000, true)]
#[case::maximum(3_600_000, true)]
#[case::truncated_second(3_600_999, true)]
#[case::above_maximum(3_601_000, false)]
#[tokio::test]
async fn new_admission_preserves_deadline_second_precision_and_bounds(
	mut manifest: Manifest,
	validation: DefinitionValidation,
	#[case] milliseconds: i64,
	#[case] accepted: bool,
) {
	let mut adapter = Adapter::new();
	manifest.deadline = adapter.now() + Duration::milliseconds(milliseconds);
	let result = submit_in(&mut adapter, &validation, &manifest, None).await;
	if accepted {
		assert_eq!(result.unwrap(), status(&manifest));
	} else {
		assert!(
			matches!(result, Err(Error::Invalid(message)) if message == "new transaction deadline must be within the next hour")
		);
		assert_eq!(adapter.log(), ["status"]);
	}
}

#[rstest]
#[tokio::test]
async fn immutable_replay_checks_origin_before_manifest_and_skips_expired_deadline(
	mut manifest: Manifest,
	validation: DefinitionValidation,
	origin: Origin,
) {
	let mut adapter = Adapter::new();
	manifest.deadline = adapter.now() - Duration::days(1);
	let stored = status(&manifest);
	{
		let mut state = adapter.0.lock().unwrap();
		state.status = Some(stored.clone());
		state.origin = Some(origin.clone());
		state.trusted = false;
	}
	assert_eq!(
		submit_in(&mut adapter, &validation, &manifest, Some(&origin))
			.await
			.unwrap(),
		stored
	);
	assert_eq!(adapter.log(), ["status", "match_origin"]);
}

#[rstest]
#[case::digest("digest")]
#[case::manifest("manifest")]
#[tokio::test]
async fn immutable_replay_cannot_change_digest_or_manifest(
	manifest: Manifest,
	validation: DefinitionValidation,
	#[case] changed: &str,
) {
	let mut adapter = Adapter::new();
	let mut stored = status(&manifest);
	if changed == "digest" {
		stored.digest = "different".into();
	} else {
		stored.manifest["deadline"] = json!("2031-01-01T00:00:00Z");
	}
	adapter.0.lock().unwrap().status = Some(stored);
	assert!(
		matches!(submit_in(&mut adapter, &validation, &manifest, None).await, Err(Error::Conflict(message)) if message == "transaction ID already has another immutable manifest")
	);
	assert_eq!(adapter.log(), ["status", "match_origin"]);
}

#[rstest]
#[tokio::test]
async fn replay_never_rebinds_another_origin(
	manifest: Manifest,
	validation: DefinitionValidation,
	origin: Origin,
) {
	let mut adapter = Adapter::new();
	adapter.0.lock().unwrap().status = Some(status(&manifest));
	assert!(matches!(
		submit_in(&mut adapter, &validation, &manifest, Some(&origin)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(adapter.log(), ["status", "match_origin"]);
	assert_eq!(adapter.0.lock().unwrap().origin, None);
}

#[rstest]
#[case::peer_missing(true)]
#[case::trust_disabled(false)]
#[tokio::test]
async fn peer_resolution_and_trust_precede_admission(
	manifest: Manifest,
	validation: DefinitionValidation,
	#[case] missing: bool,
) {
	let mut adapter = Adapter::new();
	{
		let mut state = adapter.0.lock().unwrap();
		state.trusted = false;
		if missing {
			state.failure = Some("resolve_peer".into());
		}
	}
	let result = submit_in(&mut adapter, &validation, &manifest, None).await;
	if missing {
		assert!(matches!(result, Err(Error::External(message)) if message == "resolve_peer"));
		assert_eq!(adapter.log(), ["status", "resolve_peer"]);
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
		assert_eq!(adapter.log(), ["status", "resolve_peer", "trusted"]);
	}
}

#[rstest]
#[tokio::test]
async fn new_origin_is_bound_inside_the_same_admission_scope(
	manifest: Manifest,
	validation: DefinitionValidation,
	origin: Origin,
) {
	let mut adapter = Adapter::new();
	submit_in(&mut adapter, &validation, &manifest, Some(&origin))
		.await
		.unwrap();
	assert_eq!(
		adapter.log(),
		[
			"status",
			"resolve_peer",
			"trusted",
			"admit",
			"bind_origin",
			"match_origin"
		]
	);
	assert_eq!(adapter.0.lock().unwrap().origin.as_ref(), Some(&origin));
}

#[rstest]
#[case::wrong_coordinator(false)]
#[case::invalid_manifest(true)]
#[tokio::test]
async fn invalid_submission_never_reads_storage(
	mut manifest: Manifest,
	validation: DefinitionValidation,
	#[case] invalid: bool,
) {
	let mut adapter = Adapter::new();
	if invalid {
		manifest.id = Uuid::nil();
	} else {
		manifest.coordinator = "aidash://peer".into();
	}
	assert!(matches!(
		submit_in(&mut adapter, &validation, &manifest, None).await,
		Err(Error::Invalid(_)) | Err(Error::Domain(aidash_domain::Error::Invalid(_)))
	));
	assert_eq!(adapter.log(), Vec::<String>::new());
}

#[rstest]
#[tokio::test]
async fn standalone_submission_commits_before_waking_workers(
	manifest: Manifest,
	validation: DefinitionValidation,
) {
	let adapter = Adapter::new();
	submit(&adapter, &validation, &manifest, None)
		.await
		.unwrap();
	let log = adapter.log();
	assert_eq!(
		&log[log.len() - 4..],
		&[
			"coordinator.submit.before",
			"commit",
			"coordinator.submit.after",
			"wake"
		]
	);
}

#[rstest]
#[case::before_commit("coordinator.submit.before", false)]
#[case::commit("commit", false)]
#[case::after_commit("coordinator.submit.after", true)]
#[tokio::test]
async fn failed_submission_preserves_its_commit_boundary(
	manifest: Manifest,
	validation: DefinitionValidation,
	#[case] point: &str,
	#[case] committed: bool,
) {
	let adapter = Adapter::new();
	adapter.0.lock().unwrap().failure = Some(point.into());
	assert!(
		matches!(submit(&adapter, &validation, &manifest, None).await, Err(Error::External(message)) if message == point)
	);
	let log = adapter.log();
	assert_eq!(log.contains(&"rollback".into()), !committed);
	assert_eq!(log.contains(&"wake".into()), false);
}
