use super::*;
use aidash_domain::{Artifact, Event, Message, Task, TaskStatus, Workspace};
use async_trait::async_trait;
use chrono::Utc;
use rstest::{fixture, rstest};
use serde_json::json;
fn snapshot() -> WorkspaceSnapshot {
	WorkspaceSnapshot {
		workspace: Workspace {
			id: Uuid::from_u128(1),
			title: "workspace".into(),
			goal: "goal".into(),
			state: json!({}),
			revision: 1,
			created_at: Utc::now(),
		},
		tasks: vec![Task {
			id: Uuid::from_u128(7),
			workspace_id: Uuid::from_u128(1),
			title: "task".into(),
			description: "task".into(),
			status: TaskStatus::Open,
			requirements: json!({}),
			owner: None,
			created_by: "saved".into(),
			dependencies: vec![],
			parent_id: None,
			revision: 1,
			created_at: Utc::now(),
		}],
		artifacts: vec![Artifact {
			id: Uuid::from_u128(6),
			workspace_id: Uuid::from_u128(1),
			task_id: Uuid::from_u128(7),
			kind: "text".into(),
			name: "artifact".into(),
			content: json!("immutable"),
			created_by: "saved".into(),
			idempotency_key: "artifact".into(),
			created_at: Utc::now(),
		}],
		messages: vec![Message {
			id: Uuid::from_u128(5),
			workspace_id: Uuid::from_u128(1),
			sender: "saved".into(),
			content: "saved".into(),
			idempotency_key: None,
			created_at: Utc::now(),
		}],
		events: vec![],
	}
}
fn event(kind: &str, data: Value) -> Event {
	Event {
		sequence: 1,
		id: Uuid::from_u128(99),
		node_id: "aidash://local".into(),
		workspace_id: Some(Uuid::from_u128(1)),
		kind: kind.into(),
		data,
		created_at: Utc::now(),
	}
}
fn entry(id: &str, version: &str) -> Entry {
	serde_json::from_value(
		json!({"id":id,"version":version,"kind":"agent","name":{},"description":{}}),
	)
	.unwrap()
}
type LegacyLookup = (Option<Uuid>, Option<String>, Option<String>);
type RecordedSources = (ReadMembership, Uuid, Vec<(String, Uuid)>);
struct Scope {
	run: Option<Uuid>,
	grant: Option<Uuid>,
	calls: Vec<&'static str>,
	legacy: Vec<Uuid>,
	lookups: Vec<LegacyLookup>,
	records: Vec<RecordedSources>,
	registry: Vec<(Uuid, Vec<String>, Vec<String>)>,
	fail: Option<&'static str>,
	semantic_failure: Option<Failure>,
	semantic_denied: bool,
	sources: Vec<(Uuid, String, Uuid)>,
	source_checks: Vec<Uuid>,
	run_checks: Vec<Uuid>,
	denied_source: Option<Uuid>,
	denied_run: Option<Uuid>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		run: Some(Uuid::from_u128(42)),
		grant: None,
		calls: vec![],
		legacy: vec![
			Uuid::from_u128(15),
			Uuid::from_u128(14),
			Uuid::from_u128(15),
		],
		lookups: vec![],
		records: vec![],
		registry: vec![],
		fail: None,
		semantic_failure: None,
		semantic_denied: false,
		sources: vec![],
		source_checks: vec![],
		run_checks: vec![],
		denied_source: None,
		denied_run: None,
	}
}
impl Scope {
	fn touch(&mut self, call: &'static str) -> Result<()> {
		self.calls.push(call);
		if self.fail == Some(call) {
			Err(Error::Port(Box::new(std::io::Error::other(call))))
		} else {
			Ok(())
		}
	}
}
#[async_trait]
impl ReadJournalScope for Scope {
	fn membership(&self) -> (Option<Uuid>, Option<Uuid>) {
		(self.run, self.grant)
	}
	async fn legacy_messages(
		&mut self,
		workspace: Option<Uuid>,
		sender: Option<&str>,
		content: Option<&str>,
	) -> Result<Vec<Uuid>> {
		self.touch("legacy")?;
		self.lookups.push((
			workspace,
			sender.map(str::to_owned),
			content.map(str::to_owned),
		));
		Ok(self.legacy.clone())
	}
	async fn record_sources(
		&mut self,
		membership: ReadMembership,
		workspace: Uuid,
		sources: &[(String, Uuid)],
	) -> Result<()> {
		self.touch("sources_commit")?;
		self.records.push((membership, workspace, sources.to_vec()));
		Ok(())
	}
	async fn record_registry(
		&mut self,
		run: Uuid,
		ids: Vec<String>,
		versions: Vec<String>,
	) -> Result<()> {
		self.touch("registry_commit")?;
		self.registry.push((run, ids, versions));
		Ok(())
	}
}
#[async_trait]
impl GrantJournalScope for Scope {
	async fn remote_semantic_sources(&mut self, grant: Uuid) -> Result<()> {
		assert_eq!(grant, Uuid::from_u128(8));
		self.touch("semantic")?;
		if self.semantic_denied {
			return Err(Error::Forbidden);
		}
		if let Some(failure) = self.semantic_failure {
			return Err(Error::RemoteSemantic(failure));
		}
		Ok(())
	}
	async fn sources(&mut self, grant: Uuid) -> Result<Vec<(Uuid, String, Uuid)>> {
		assert_eq!(grant, Uuid::from_u128(8));
		self.touch("grant_sources")?;
		Ok(self.sources.clone())
	}
	async fn source_visible(
		&mut self,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
		pending: &mut Vec<Uuid>,
	) -> Result<bool> {
		assert_eq!(workspace, Uuid::from_u128(1));
		self.touch("source_policy")?;
		self.source_checks.push(id);
		if kind == "run" {
			pending.push(id);
		}
		Ok(self.denied_source != Some(id))
	}
	async fn run_reads(&mut self, run: Uuid) -> Result<bool> {
		self.touch("run_reads")?;
		self.run_checks.push(run);
		Ok(self.denied_run != Some(run))
	}
}
#[rstest]
#[case::local(false)]
#[case::remote(true)]
#[tokio::test]
async fn snapshot_records_one_sorted_unique_membership_commit(
	mut scope: Scope,
	#[case] remote: bool,
) {
	if remote {
		scope.run = None;
		scope.grant = Some(Uuid::from_u128(8));
	}
	let mut snapshot = snapshot();
	snapshot.tasks.push(snapshot.tasks[0].clone());
	snapshot.messages.push(snapshot.messages[0].clone());
	snapshot.events = vec![event("message.created", json!({"id":Uuid::from_u128(5)}))];
	track_snapshot(&mut scope, &snapshot).await.unwrap();
	assert_eq!(scope.calls, vec!["sources_commit"]);
	assert_eq!(
		scope.records,
		vec![(
			if remote {
				ReadMembership::RemoteGrant(Uuid::from_u128(8))
			} else {
				ReadMembership::Run(Uuid::from_u128(42))
			},
			Uuid::from_u128(1),
			vec![
				("artifact".into(), Uuid::from_u128(6)),
				("message".into(), Uuid::from_u128(5)),
				("task".into(), Uuid::from_u128(7)),
				("workspace_events".into(), Uuid::from_u128(1))
			]
		)]
	);
}
#[rstest]
#[tokio::test]
async fn callers_without_read_membership_do_not_resolve_or_write_journals(mut scope: Scope) {
	scope.run = None;
	let mut snapshot = snapshot();
	snapshot.events = vec![event(
		"message.created",
		json!({"sender":"saved","content":"old"}),
	)];
	track_snapshot(&mut scope, &snapshot).await.unwrap();
	assert!(scope.calls.is_empty());
}
#[rstest]
#[tokio::test]
async fn conflicting_local_and_remote_membership_denies_before_any_journal_io(mut scope: Scope) {
	scope.grant = Some(Uuid::from_u128(8));
	assert!(matches!(
		track_snapshot(&mut scope, &snapshot()).await,
		Err(Error::Forbidden)
	));
	assert!(scope.calls.is_empty());
}
#[rstest]
#[case::conversation("conversation.started", "conversation")]
#[case::conversation_end("conversation.completed", "conversation")]
#[case::generation("generation.started", "generation")]
#[case::run_created("run.created", "run")]
#[case::unknown("extension.created", "run")]
#[tokio::test]
async fn event_membership_keeps_the_original_source_selector(
	mut scope: Scope,
	#[case] kind: &str,
	#[case] expected: &str,
) {
	let mut snapshot = snapshot();
	snapshot.events = vec![event(
		kind,
		json!({"id":Uuid::from_u128(12),"run_id":Uuid::from_u128(13)}),
	)];
	track_snapshot(&mut scope, &snapshot).await.unwrap();
	let expected_id = if kind == "extension.created" { 13 } else { 12 };
	assert!(
		scope.records[0]
			.2
			.contains(&(expected.into(), Uuid::from_u128(expected_id)))
	);
	assert!(!scope.records[0].2.contains(&(
		expected.into(),
		Uuid::from_u128(if expected_id == 12 { 13 } else { 12 })
	)));
}
#[rstest]
#[case::local(false)]
#[case::remote(true)]
#[tokio::test]
async fn only_the_current_local_run_is_excluded_from_recursive_membership(
	mut scope: Scope,
	#[case] remote: bool,
) {
	if remote {
		scope.run = None;
		scope.grant = Some(Uuid::from_u128(8));
	}
	let mut snapshot = snapshot();
	snapshot.events = vec![
		event("run.created", json!({"id":Uuid::from_u128(42)})),
		event("run.phase", json!({"run_id":Uuid::from_u128(43)})),
	];
	track_snapshot(&mut scope, &snapshot).await.unwrap();
	assert_eq!(
		scope.records[0]
			.2
			.contains(&("run".into(), Uuid::from_u128(42))),
		remote
	);
	assert!(
		scope.records[0]
			.2
			.contains(&("run".into(), Uuid::from_u128(43)))
	);
}
#[rstest]
#[case::absent(json!({}))]
#[case::malformed(json!({"id":"invalid","run_id":"invalid"}))]
#[case::non_string(json!({"id":42,"run_id":43}))]
#[tokio::test]
async fn malformed_event_references_keep_workspace_events_without_inventing_sources(
	mut scope: Scope,
	#[case] data: Value,
) {
	let mut snapshot = snapshot();
	snapshot.events = vec![event("conversation.started", data)];
	track_snapshot(&mut scope, &snapshot).await.unwrap();
	assert_eq!(scope.records[0].2.len(), 4);
	assert!(
		scope.records[0]
			.2
			.contains(&("workspace_events".into(), Uuid::from_u128(1)))
	);
}
#[rstest]
#[tokio::test]
async fn legacy_message_membership_keeps_every_matching_immutable_row(mut scope: Scope) {
	let mut snapshot = snapshot();
	snapshot.events = vec![event(
		"message.created",
		json!({"sender":"saved","content":"identical"}),
	)];
	track_snapshot(&mut scope, &snapshot).await.unwrap();
	assert_eq!(scope.calls, vec!["legacy", "sources_commit"]);
	assert_eq!(
		scope.lookups,
		vec![(
			Some(Uuid::from_u128(1)),
			Some("saved".into()),
			Some("identical".into())
		)]
	);
	assert!(
		scope.records[0]
			.2
			.contains(&("message".into(), Uuid::from_u128(14)))
	);
	assert!(
		scope.records[0]
			.2
			.contains(&("message".into(), Uuid::from_u128(15)))
	);
	assert_eq!(scope.records[0].2.len(), 6);
}
#[rstest]
#[tokio::test]
async fn legacy_message_null_fields_are_forwarded_without_normalization(mut scope: Scope) {
	let mut snapshot = snapshot();
	let mut old = event("message.created", json!({"sender":9,"content":null}));
	old.workspace_id = None;
	snapshot.events = vec![old];
	track_snapshot(&mut scope, &snapshot).await.unwrap();
	assert_eq!(scope.lookups, vec![(None, None, None)]);
}
#[rstest]
#[tokio::test]
async fn thread_open_events_without_a_message_id_do_not_use_legacy_content_matching(
	mut scope: Scope,
) {
	let mut snapshot = snapshot();
	snapshot.events = vec![event(
		"message.thread_opened",
		json!({"sender":"saved","content":"old"}),
	)];
	track_snapshot(&mut scope, &snapshot).await.unwrap();
	assert!(scope.lookups.is_empty());
	assert_eq!(scope.records[0].2.len(), 4);
}
#[rstest]
#[tokio::test]
async fn empty_membership_sources_keep_the_existing_commit_boundary(mut scope: Scope) {
	let mut snapshot = snapshot();
	snapshot.tasks.clear();
	snapshot.artifacts.clear();
	snapshot.messages.clear();
	track_snapshot(&mut scope, &snapshot).await.unwrap();
	assert_eq!(scope.calls, vec!["sources_commit"]);
	assert!(scope.records[0].2.is_empty());
}
#[rstest]
#[case::lookup("legacy")]
#[case::commit("sources_commit")]
#[tokio::test]
async fn membership_errors_cannot_return_a_completed_disclosure(
	mut scope: Scope,
	#[case] fail: &'static str,
) {
	scope.fail = Some(fail);
	let mut snapshot = snapshot();
	snapshot.events = vec![event("message.created", json!({}))];
	let Error::Port(error) = track_snapshot(&mut scope, &snapshot).await.unwrap_err() else {
		panic!("expected opaque error");
	};
	assert!(error.is::<std::io::Error>());
	assert!(scope.records.is_empty());
	assert_eq!(scope.calls.last(), Some(&fail));
}
#[rstest]
#[tokio::test]
async fn registry_membership_keeps_id_version_pairs_and_duplicate_input_order(mut scope: Scope) {
	let entries = vec![
		entry("z", "2.0.0"),
		entry("a", "1.0.0"),
		entry("z", "2.0.0"),
	];
	track_registry(&mut scope, &entries).await.unwrap();
	assert_eq!(
		scope.registry,
		vec![(
			Uuid::from_u128(42),
			vec!["z".into(), "a".into(), "z".into()],
			vec!["2.0.0".into(), "1.0.0".into(), "2.0.0".into()]
		)]
	);
}
#[rstest]
#[case::operator(None, None, false)]
#[case::grant(None, Some(Uuid::from_u128(8)), false)]
#[case::run(Some(Uuid::from_u128(42)), None, true)]
#[case::both(Some(Uuid::from_u128(42)), Some(Uuid::from_u128(8)), true)]
#[tokio::test]
async fn registry_tracking_preserves_its_local_run_only_contract(
	mut scope: Scope,
	#[case] run: Option<Uuid>,
	#[case] grant: Option<Uuid>,
	#[case] committed: bool,
) {
	scope.run = run;
	scope.grant = grant;
	track_registry(&mut scope, &[]).await.unwrap();
	assert_eq!(scope.registry.len(), usize::from(committed));
	assert_eq!(scope.calls.len(), usize::from(committed));
}
#[rstest]
#[tokio::test]
async fn registry_write_failure_retains_its_error_identity(mut scope: Scope) {
	scope.fail = Some("registry_commit");
	let Error::Port(error) = track_registry(&mut scope, &[entry("a", "1.0.0")])
		.await
		.unwrap_err()
	else {
		panic!("expected opaque error");
	};
	assert!(error.is::<std::io::Error>());
	assert!(scope.registry.is_empty());
}
#[rstest]
#[case::policy(true, None)]
#[case::invalidated(false, Some(Failure::Invalidated))]
#[tokio::test]
async fn remote_semantic_denial_stops_before_grant_source_disclosure(
	mut scope: Scope,
	#[case] denied: bool,
	#[case] failure: Option<Failure>,
) {
	scope.semantic_denied = denied;
	scope.semantic_failure = failure;
	assert!(
		!grant_reads_visible(&mut scope, Uuid::from_u128(8))
			.await
			.unwrap()
	);
	assert_eq!(scope.calls, vec!["semantic"]);
}
#[rstest]
#[case::configuration(Failure::Configuration)]
#[case::authority(Failure::Authority)]
#[case::provider(Failure::ProviderContract)]
#[case::budget(Failure::ContextBudget)]
#[case::allowance(Failure::Allowance)]
#[case::unavailable(Failure::Unavailable)]
#[case::exhausted(Failure::RetriesExhausted)]
#[case::pending(Failure::Pending)]
#[tokio::test]
async fn other_remote_semantic_recovery_reasons_keep_their_exact_error(
	mut scope: Scope,
	#[case] failure: Failure,
) {
	scope.semantic_failure = Some(failure);
	let Error::RemoteSemantic(actual) = grant_reads_visible(&mut scope, Uuid::from_u128(8))
		.await
		.unwrap_err()
	else {
		panic!("expected semantic recovery error");
	};
	assert_eq!(actual, failure);
	assert_eq!(scope.calls, vec!["semantic"]);
}
fn grant_sources() -> Vec<(Uuid, String, Uuid)> {
	vec![
		(Uuid::from_u128(1), "run".into(), Uuid::from_u128(12)),
		(Uuid::from_u128(1), "task".into(), Uuid::from_u128(13)),
		(Uuid::from_u128(1), "run".into(), Uuid::from_u128(14)),
	]
}
#[rstest]
#[tokio::test]
async fn grant_sources_are_all_authorized_before_following_any_run_journal(mut scope: Scope) {
	scope.sources = grant_sources();
	assert!(
		grant_reads_visible(&mut scope, Uuid::from_u128(8))
			.await
			.unwrap()
	);
	assert_eq!(
		scope.source_checks,
		vec![
			Uuid::from_u128(12),
			Uuid::from_u128(13),
			Uuid::from_u128(14)
		]
	);
	assert_eq!(
		scope.run_checks,
		vec![Uuid::from_u128(12), Uuid::from_u128(14)]
	);
	assert_eq!(
		scope.calls,
		vec![
			"semantic",
			"grant_sources",
			"source_policy",
			"source_policy",
			"source_policy",
			"run_reads",
			"run_reads"
		]
	);
}
#[rstest]
#[tokio::test]
async fn a_denied_grant_source_stops_before_run_dependencies(mut scope: Scope) {
	scope.sources = grant_sources();
	scope.denied_source = Some(Uuid::from_u128(13));
	assert!(
		!grant_reads_visible(&mut scope, Uuid::from_u128(8))
			.await
			.unwrap()
	);
	assert_eq!(
		scope.source_checks,
		vec![Uuid::from_u128(12), Uuid::from_u128(13)]
	);
	assert!(scope.run_checks.is_empty());
}
#[rstest]
#[tokio::test]
async fn a_denied_run_dependency_stops_before_later_run_reads(mut scope: Scope) {
	scope.sources = grant_sources();
	scope.denied_run = Some(Uuid::from_u128(12));
	assert!(
		!grant_reads_visible(&mut scope, Uuid::from_u128(8))
			.await
			.unwrap()
	);
	assert_eq!(scope.source_checks.len(), 3);
	assert_eq!(scope.run_checks, vec![Uuid::from_u128(12)]);
}
#[rstest]
#[case::semantic("semantic")]
#[case::rows("grant_sources")]
#[case::source_policy("source_policy")]
#[case::run_reads("run_reads")]
#[tokio::test]
async fn grant_adapter_errors_keep_their_opaque_identity(
	mut scope: Scope,
	#[case] fail: &'static str,
) {
	scope.sources = grant_sources();
	scope.fail = Some(fail);
	let Error::Port(error) = grant_reads_visible(&mut scope, Uuid::from_u128(8))
		.await
		.unwrap_err()
	else {
		panic!("expected opaque error");
	};
	assert!(error.is::<std::io::Error>());
	assert_eq!(scope.calls.last(), Some(&fail));
}

#[rstest]
#[case::local(false)]
#[case::remote(true)]
#[tokio::test]
async fn child_task_membership_keeps_input_order_and_the_same_commit(
	mut scope: Scope,
	#[case] remote: bool,
) {
	if remote {
		scope.run = None;
		scope.grant = Some(Uuid::from_u128(8));
	}
	track_tasks(
		&mut scope,
		Uuid::from_u128(1),
		&[Uuid::from_u128(3), Uuid::from_u128(2), Uuid::from_u128(3)],
	)
	.await
	.unwrap();
	assert_eq!(scope.calls, vec!["sources_commit"]);
	assert_eq!(
		scope.records[0].2,
		vec![
			("task".into(), Uuid::from_u128(3)),
			("task".into(), Uuid::from_u128(2)),
			("task".into(), Uuid::from_u128(3))
		]
	);
	assert_eq!(
		scope.records[0].0,
		if remote {
			ReadMembership::RemoteGrant(Uuid::from_u128(8))
		} else {
			ReadMembership::Run(Uuid::from_u128(42))
		}
	);
}
#[rstest]
#[tokio::test]
async fn empty_child_page_keeps_the_existing_no_commit_boundary_even_with_conflicting_scopes(
	mut scope: Scope,
) {
	scope.grant = Some(Uuid::from_u128(8));
	track_tasks(&mut scope, Uuid::from_u128(1), &[])
		.await
		.unwrap();
	assert!(scope.calls.is_empty());
	assert!(matches!(
		track_tasks(&mut scope, Uuid::from_u128(1), &[Uuid::from_u128(2)]).await,
		Err(Error::Forbidden)
	));
	assert!(scope.calls.is_empty());
}
#[rstest]
#[tokio::test]
async fn child_membership_write_failure_cannot_return_success(mut scope: Scope) {
	scope.fail = Some("sources_commit");
	let Error::Port(error) = track_tasks(&mut scope, Uuid::from_u128(1), &[Uuid::from_u128(2)])
		.await
		.unwrap_err()
	else {
		panic!("expected opaque error");
	};
	assert!(error.is::<std::io::Error>());
	assert!(scope.records.is_empty());
}
