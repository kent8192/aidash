use super::*;
use crate::{
	authorization::Snapshot,
	ports::federation::foreign_reads::{AdmissionEvidence, ForeignSourceAuthority},
};
use aidash_domain::{
	RunPhase, Task, TaskStatus,
	federation::execution::{Definition, Description, Inspection},
	policy::{PolicyBundle, Resource},
	registry::{EntityRef, Entry},
	semantic::remote::{Binding, Provider},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone, Copy)]
enum Fault {
	Forbidden,
	Unauthorized,
	Missing,
	Storage,
}
impl Fault {
	fn error(self) -> Error {
		match self {
			Self::Forbidden => Error::Forbidden,
			Self::Unauthorized => Error::Unauthorized,
			Self::Missing => Error::NotFound("definition".into()),
			Self::Storage => Error::External("storage failed".into()),
		}
	}
}

struct Scope {
	calls: Vec<String>,
	now: DateTime<Utc>,
	record: Option<AdmissionEvidence>,
	mapped: Option<Uuid>,
	snapshot: Snapshot,
	source_snapshot: Snapshot,
	subjects: Vec<String>,
	view_denied: Option<&'static str>,
	source_denied: Option<&'static str>,
	snapshot_fault: Option<Fault>,
	entry_fault: Option<Fault>,
	entry: Entry,
	retired: Option<Entry>,
	terminal_statuses: Vec<String>,
	block_source: bool,
	source_executor_enabled: Option<bool>,
}

#[async_trait]
impl ForeignRunReadScope for Scope {
	fn node(&self) -> &str {
		"aidash://local"
	}
	fn now(&self) -> DateTime<Utc> {
		self.now
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: "acme".into(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		assert_eq!(self.snapshot.revision, 1);
		assert_eq!(self.subjects, ["viewer"]);
		assert_eq!(resource.tenant, "acme");
		self.calls.push(format!("viewer:{action}"));
		Ok(self.view_denied != Some(action))
	}
	async fn admission(&mut self, _: &RunMetadata) -> Result<Option<AdmissionEvidence>> {
		self.calls.push("admission".into());
		Ok(self.record.clone())
	}
	async fn mapped_credential(
		&mut self,
		_: &RunMetadata,
		_: &Description,
	) -> Result<Option<Uuid>> {
		self.calls.push("mapping".into());
		Ok(self.mapped)
	}
	async fn source_snapshot(&mut self, credential: Uuid, root: &str) -> Result<Snapshot> {
		assert_eq!(credential, Uuid::from_u128(9));
		assert_eq!(root, "root");
		self.calls.push("source-snapshot".into());
		match self.snapshot_fault {
			Some(fault) => Err(fault.error()),
			None => Ok(self.source_snapshot.clone()),
		}
	}
	async fn retired_definition(
		&mut self,
		_: &RunMetadata,
		_: &Description,
		credential: Uuid,
		statuses: &[&str],
	) -> Result<Option<Entry>> {
		assert_eq!(credential, Uuid::from_u128(9));
		self.calls.push("retirement-fence".into());
		self.terminal_statuses = statuses.iter().map(|s| (*s).into()).collect();
		Ok(self.retired.clone())
	}
	fn source_authority(
		&mut self,
		snapshot: Snapshot,
		subjects: Vec<String>,
	) -> Box<dyn ForeignSourceAuthority + '_> {
		self.source_executor_enabled = snapshot
			.bundle
			.subjects
			.get("aidash://local/agents/a@1.0.0")
			.map(|s| s.enabled);
		let viewer = std::mem::replace(&mut self.snapshot, snapshot);
		let viewer_subjects = std::mem::replace(&mut self.subjects, subjects);
		self.calls.push("source-lease".into());
		Box::new(Source {
			scope: self,
			viewer: Some(viewer),
			viewer_subjects: Some(viewer_subjects),
		})
	}
}
struct Source<'a> {
	scope: &'a mut Scope,
	viewer: Option<Snapshot>,
	viewer_subjects: Option<Vec<String>>,
}
impl Drop for Source<'_> {
	fn drop(&mut self) {
		if let Some(viewer) = self.viewer.take() {
			self.scope.snapshot = viewer;
		}
		if let Some(subjects) = self.viewer_subjects.take() {
			self.scope.subjects = subjects;
		}
		self.scope.calls.push("restore-viewer".into());
	}
}
#[async_trait]
impl ForeignSourceAuthority for Source<'_> {
	fn catalog_resource(&self, entry: &Entry) -> Resource {
		self.scope
			.resource("registry", &entry.id, json!({"version":entry.version}))
	}
	async fn decide(&mut self, _: &Resource, action: &str) -> Result<bool> {
		assert_eq!(self.scope.snapshot.revision, 99);
		self.scope.calls.push(format!("source:{action}"));
		if self.scope.block_source {
			std::future::pending::<()>().await;
		}
		Ok(self.scope.source_denied != Some(action))
	}
	async fn catalog_entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		assert_eq!(reference.id, "a");
		assert_eq!(action, "registry.read");
		self.scope.calls.push("current-definition".into());
		match self.scope.entry_fault {
			Some(fault) => Err(fault.error()),
			None => Ok(self.scope.entry.clone()),
		}
	}
}

#[fixture]
fn run() -> RunMetadata {
	serde_json::from_value(json!({"id":Uuid::from_u128(1),"task_id":Uuid::from_u128(2),"workspace_id":Uuid::from_u128(3),
        "home_node":"aidash://home","agent_id":"a","agent_version":"1.0.0","phase":"THINKING","control":"ACTIVE",
        "step":0,"revision":1,"observed_input_seq":0,"ledger_worker_ready":false,"error":null,"lease_owner":null,"lease_until":null,"updated_at":"2026-10-03T00:00:00Z"})).unwrap()
}
#[fixture]
fn scope(run: RunMetadata) -> Scope {
	let now = DateTime::parse_from_rfc3339("2026-10-03T00:00:00Z")
		.unwrap()
		.with_timezone(&Utc);
	let entry: Entry = serde_json::from_value(
		json!({"id":"a","version":"1.0.0","kind":"agent","name":{"en":"A"},"description":{},
        "config":{"model":{"id":"m","version":"1.0.0"},"instructions":"Do work."}}),
	)
	.unwrap();
	let description = Description {
		grant_id: Uuid::from_u128(4),
		source_node: run.home_node.clone(),
		target_node: "aidash://local".into(),
		source_tenant: "home-tenant".into(),
		source_subject: "home-reader".into(),
		task: Task {
			id: run.task_id,
			workspace_id: run.workspace_id,
			title: "Task".into(),
			description: "".into(),
			status: TaskStatus::Running,
			requirements: json!({}),
			owner: None,
			created_by: "creator".into(),
			dependencies: vec![],
			parent_id: None,
			revision: 1,
			created_at: now,
		},
		inspection: Inspection {
			binding_snapshot: crate::test_support::snapshot("aidash://receiver", "agent"),
			generation: None,
			lineage: vec![],
			node_id: "aidash://local".into(),
			authority_digest: format!("sha256:{}", "a".repeat(64)),
			agent: entry.clone(),
			definitions: vec![Definition {
				entry: EntityRef {
					id: entry.id.clone(),
					version: entry.version.clone(),
				},
				kind: entry.kind.clone(),
				digest: digest(&serde_json::to_value(&entry).unwrap()),
				metadata: entry.clone(),
			}],
			semantic_memory: 0,
			compactor: None,
		},
		expires_at: now + chrono::Duration::minutes(10),
		semantic: Binding::Disabled {},
	};
	let bundle: PolicyBundle = serde_json::from_value(json!({"tenant":"acme","subjects":{
        "root":{"kind":"user"},"aidash://local/agents/a@1.0.0":{"kind":"agent"}}}))
	.unwrap();
	Scope {
		calls: vec![],
		now,
		record: Some(AdmissionEvidence {
			credential_id: Uuid::from_u128(9),
			subjects: vec!["root".into(), "aidash://local/agents/a@1.0.0".into()],
			description,
		}),
		mapped: Some(Uuid::from_u128(9)),
		snapshot: Snapshot {
			revision: 1,
			bundle: bundle.clone(),
		},
		source_snapshot: Snapshot {
			revision: 99,
			bundle,
		},
		subjects: vec!["viewer".into()],
		view_denied: None,
		source_denied: None,
		snapshot_fault: None,
		entry_fault: None,
		entry,
		retired: None,
		terminal_statuses: vec![],
		block_source: false,
		source_executor_enabled: None,
	}
}
fn restored(scope: &Scope) {
	assert_eq!(scope.snapshot.revision, 1);
	assert_eq!(scope.subjects, ["viewer"]);
	assert_eq!(
		scope.calls.last().map(String::as_str),
		Some("restore-viewer")
	);
}

#[rstest]
#[tokio::test]
async fn disclosure_checks_both_authorities_and_restores_the_viewer(
	mut scope: Scope,
	run: RunMetadata,
) {
	assert!(visible(&mut scope, &run).await.unwrap());
	assert_eq!(
		scope.calls,
		vec![
			"admission",
			"viewer:workspace.read",
			"viewer:task.read",
			"viewer:run.read",
			"viewer:memory.read",
			"mapping",
			"source-snapshot",
			"source-lease",
			"source:workspace.read",
			"source:task.read",
			"source:task.execute",
			"current-definition",
			"source:agent.execute",
			"restore-viewer"
		]
	);
	restored(&scope);
}
#[rstest]
#[case("workspace.read")]
#[case("task.read")]
#[case("run.read")]
#[case("memory.read")]
#[tokio::test]
async fn viewer_denials_stop_before_source_authority(
	mut scope: Scope,
	run: RunMetadata,
	#[case] denied: &'static str,
) {
	scope.view_denied = Some(denied);
	assert!(!visible(&mut scope, &run).await.unwrap());
	assert!(
		!scope
			.calls
			.iter()
			.any(|c| c == "mapping" || c == "source-lease")
	);
}
#[rstest]
#[case("source")]
#[case("target")]
#[case("task")]
#[case("workspace")]
#[case("agent")]
#[case("version")]
#[case("expired")]
#[tokio::test]
async fn rebound_or_expired_descriptions_fail_before_viewer_checks(
	mut scope: Scope,
	run: RunMetadata,
	#[case] mutation: &str,
) {
	let d = &mut scope.record.as_mut().unwrap().description;
	match mutation {
		"source" => d.source_node = "aidash://other".into(),
		"target" => d.target_node = "aidash://other".into(),
		"task" => d.task.id = Uuid::nil(),
		"workspace" => d.task.workspace_id = Uuid::nil(),
		"agent" => d.inspection.agent.id = "other".into(),
		"version" => d.inspection.agent.version = "2.0.0".into(),
		"expired" => d.expires_at = scope.now,
		_ => panic!("unknown mutation"),
	}
	assert!(!visible(&mut scope, &run).await.unwrap());
	assert_eq!(scope.calls, ["admission"]);
}
#[rstest]
#[case(Fault::Forbidden)]
#[case(Fault::Unauthorized)]
#[tokio::test]
async fn revoked_source_identity_denies_disclosure(
	mut scope: Scope,
	run: RunMetadata,
	#[case] fault: Fault,
) {
	scope.snapshot_fault = Some(fault);
	assert!(!visible(&mut scope, &run).await.unwrap());
	assert!(!scope.calls.iter().any(|c| c == "source-lease"));
}
#[rstest]
#[tokio::test]
async fn source_storage_failures_propagate(mut scope: Scope, run: RunMetadata) {
	scope.snapshot_fault = Some(Fault::Storage);
	assert!(
		matches!(visible(&mut scope,&run).await,Err(Error::External(ref message)) if message=="storage failed")
	);
}
#[rstest]
#[case("workspace.read")]
#[case("task.read")]
#[case("task.execute")]
#[case("agent.execute")]
#[tokio::test]
async fn source_denials_restore_authority_before_returning(
	mut scope: Scope,
	run: RunMetadata,
	#[case] denied: &'static str,
) {
	scope.source_denied = Some(denied);
	assert!(!visible(&mut scope, &run).await.unwrap());
	restored(&scope);
}
#[rstest]
#[case(Fault::Forbidden)]
#[case(Fault::Missing)]
#[tokio::test]
async fn hidden_current_definitions_deny_and_restore(
	mut scope: Scope,
	run: RunMetadata,
	#[case] fault: Fault,
) {
	scope.entry_fault = Some(fault);
	assert!(!visible(&mut scope, &run).await.unwrap());
	restored(&scope);
}
#[rstest]
#[case(Fault::Unauthorized)]
#[case(Fault::Storage)]
#[tokio::test]
async fn other_catalog_failures_keep_their_category_and_restore(
	mut scope: Scope,
	run: RunMetadata,
	#[case] fault: Fault,
) {
	scope.entry_fault = Some(fault);
	assert!(visible(&mut scope, &run).await.is_err());
	restored(&scope);
}
#[rstest]
#[tokio::test]
async fn definition_digest_changes_deny_without_executing_the_definition(
	mut scope: Scope,
	run: RunMetadata,
) {
	scope
		.entry
		.description
		.insert("en".into(), "Changed".into());
	assert!(!visible(&mut scope, &run).await.unwrap());
	assert!(!scope.calls.iter().any(|c| c == "source:agent.execute"));
	restored(&scope);
}
#[rstest]
#[tokio::test]
async fn cancellation_drops_the_source_authority_lease(mut scope: Scope, run: RunMetadata) {
	scope.block_source = true;
	assert!(
		tokio::time::timeout(
			std::time::Duration::from_millis(10),
			visible(&mut scope, &run)
		)
		.await
		.is_err()
	);
	restored(&scope);
}
#[rstest]
#[case(RunPhase::Completed,RunControl::Active,vec!["COMPLETED"])]
#[case(RunPhase::Failed,RunControl::Active,vec!["FAILED"])]
#[case(RunPhase::Cancelled,RunControl::Active,vec!["STOPPED","EXPIRED"])]
#[case(RunPhase::Thinking,RunControl::Cancelled,vec!["STOPPED","EXPIRED"])]
#[tokio::test]
async fn terminal_generation_reads_require_the_exact_retirement_status_fence(
	mut scope: Scope,
	mut run: RunMetadata,
	#[case] phase: RunPhase,
	#[case] control: RunControl,
	#[case] statuses: Vec<&str>,
) {
	run.phase = phase;
	run.control = control;
	scope
		.record
		.as_mut()
		.unwrap()
		.description
		.inspection
		.generation = Some(json!({"intent":"pinned"}));
	scope.retired = Some(scope.entry.clone());
	scope
		.source_snapshot
		.bundle
		.subjects
		.get_mut("aidash://local/agents/a@1.0.0")
		.unwrap()
		.enabled = false;
	assert!(visible(&mut scope, &run).await.unwrap());
	assert_eq!(scope.terminal_statuses, statuses);
	assert_eq!(scope.source_executor_enabled, Some(true));
	assert!(!scope.source_snapshot.bundle.subjects["aidash://local/agents/a@1.0.0"].enabled);
	assert!(!scope.calls.iter().any(|c| c == "current-definition"));
	restored(&scope);
}
#[rstest]
#[case("missing")]
#[case("kind")]
#[case("chain")]
#[tokio::test]
async fn retirement_cannot_enable_an_unrelated_subject(
	mut scope: Scope,
	mut run: RunMetadata,
	#[case] mutation: &str,
) {
	run.phase = RunPhase::Completed;
	scope
		.record
		.as_mut()
		.unwrap()
		.description
		.inspection
		.generation = Some(json!({"intent":"pinned"}));
	scope.retired = Some(scope.entry.clone());
	match mutation {
		"missing" => {
			scope
				.source_snapshot
				.bundle
				.subjects
				.remove("aidash://local/agents/a@1.0.0");
		}
		"kind" => {
			scope
				.source_snapshot
				.bundle
				.subjects
				.get_mut("aidash://local/agents/a@1.0.0")
				.unwrap()
				.kind = SubjectKind::User
		}
		"chain" => {
			scope.record.as_mut().unwrap().subjects.pop();
		}
		_ => panic!("unknown mutation"),
	}
	assert!(!visible(&mut scope, &run).await.unwrap());
	assert!(!scope.calls.iter().any(|c| c == "source-lease"));
}
#[rstest]
#[tokio::test]
async fn required_home_semantics_recheck_the_source_workspace_permission(
	mut scope: Scope,
	run: RunMetadata,
) {
	scope.record.as_mut().unwrap().description.semantic = Binding::RequiredHome {
		native: None,
		home_lineage: vec![],
		execution_lineage: vec![],
		version: 1,
		index_revision: 1,
		index_digest: "digest".into(),
		embedding: Box::new(Provider {
			node_id: "aidash://home".into(),
			entry: EntityRef {
				id: "emb".into(),
				version: "1.0.0".into(),
			},
			digest: "definition".into(),
			configuration_digest: "configuration".into(),
		}),
		compactor: None,
		summarizer: None,
	};
	scope.source_denied = Some("semantic.use");
	assert!(!visible(&mut scope, &run).await.unwrap());
	restored(&scope);
}

#[rstest]
#[case("bundle", false)]
#[case("memory", false)]
#[case("source", false)]
#[case("embedding", false)]
#[case("reranker", false)]
#[case("tokenizer", false)]
#[case("bundle", true)]
#[case("memory", true)]
#[case("source", true)]
#[case("embedding", true)]
#[case("reranker", true)]
#[case("tokenizer", true)]
#[tokio::test]
async fn binding_context_definitions_keep_current_reader_authority(
	mut scope: Scope,
	run: RunMetadata,
	#[case] kind: &str,
	#[case] denied: bool,
) {
	scope.entry.kind = kind.into();
	let definition = &mut scope
		.record
		.as_mut()
		.unwrap()
		.description
		.inspection
		.definitions[0];
	definition.kind = kind.into();
	definition.metadata = scope.entry.clone();
	definition.digest = digest(&serde_json::to_value(&scope.entry).unwrap());
	if denied {
		scope.source_denied = Some("registry.read");
	}
	assert_eq!(visible(&mut scope, &run).await.unwrap(), !denied);
	assert!(
		scope
			.calls
			.iter()
			.any(|call| call == "source:registry.read")
	);
	restored(&scope);
}
