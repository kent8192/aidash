use super::*;
use crate::ports::authorization::visibility::{
	provenance::RecordedRead, resources::ResourceVisibilityScope,
};
use aidash_domain::{
	Artifact, Conversation, HumanRequest, Message, RunMetadata, Task,
	generation::requests::Request, policy::Resource, registry::EntityRef,
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::collections::BTreeMap;
struct Scope {
	graph: BTreeMap<Uuid, Vec<RecordedRead>>,
	outputs: BTreeMap<(Uuid, String, Uuid), Vec<Uuid>>,
	invalid_runs: BTreeMap<Uuid, Failure>,
	semantic_entry: Option<(Uuid, aidash_domain::semantic::mutations::Entry)>,
	entries: Vec<EntityRef>,
	opening: Vec<Uuid>,
	calls: Vec<String>,
	decisions: Vec<Resource>,
	missing: bool,
	denied: Option<&'static str>,
	fail: Option<&'static str>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		graph: BTreeMap::new(),
		outputs: BTreeMap::new(),
		invalid_runs: BTreeMap::new(),
		semantic_entry: None,
		entries: vec![],
		opening: vec![],
		calls: vec![],
		decisions: vec![],
		missing: false,
		denied: None,
		fail: None,
	}
}
impl Scope {
	fn touch(&mut self, name: &'static str) -> Result<()> {
		self.calls.push(name.into());
		if self.fail == Some(name) {
			return Err(Error::Port(Box::new(std::io::Error::other(
				"provenance adapter fault",
			))));
		}
		Ok(())
	}
	fn lookup(&mut self, name: &'static str, id: Uuid, workspace: Uuid) -> Result<()> {
		self.touch(name)?;
		self.calls.push(format!("lookup:{name}:{id}:{workspace}"));
		Ok(())
	}
}
#[async_trait]
impl ResourceVisibilityScope for Scope {
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.touch("workspace")?;
		Ok(self.resource("workspace", &id.to_string(), json!({"workspace_id":id})))
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		let key = match action {
			"workspace.events" => "workspace.events",
			"task.read" => "task.read",
			"artifact.read" => "artifact.read",
			"message.read" => "message.read",
			"conversation.read" => "conversation.read",
			_ => panic!("unexpected action"),
		};
		self.touch(key)?;
		self.decisions.push(resource.clone());
		Ok(self.denied != Some(key))
	}
	async fn artifact_task(&mut self, a: &Artifact) -> Result<Option<Task>> {
		self.lookup("artifact_task", a.task_id, a.workspace_id)?;
		let mut t = task();
		t.id = a.task_id;
		t.workspace_id = a.workspace_id;
		Ok(Some(t))
	}
	async fn output_visible(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<bool> {
		self.touch("output")?;
		Ok(self.denied != Some("output")
			&& self
				.outputs
				.get(&(workspace, kind.to_owned(), id))
				.is_none_or(|producers| {
					producers
						.iter()
						.all(|run| !self.invalid_runs.contains_key(run))
				}))
	}
	fn cached_human(&self, _: Uuid) -> Option<bool> {
		panic!("journal traversal does not directly cache human requests")
	}
	fn remember_human(&mut self, _: Uuid, _: bool) {
		panic!("journal traversal does not directly cache human requests")
	}
	async fn humans(&mut self, _: Uuid, _: Uuid) -> Result<Vec<HumanRequest>> {
		panic!("human membership is checked by the base Run reader")
	}
}
#[async_trait]
impl ReadProvenanceScope for Scope {
	async fn open_run_reads(&mut self, run: Uuid) -> Result<bool> {
		let opened = self.opening.contains(&run);
		if opened {
			self.calls.push(format!("open:{run}"));
		}
		Ok(opened)
	}
	fn close_run_reads(&mut self) {
		self.calls.push("close".into());
	}
	async fn output_producers(
		&mut self,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<Option<Vec<Uuid>>> {
		self.touch("output")?;
		Ok((self.denied != Some("output")).then(|| {
			self.outputs
				.get(&(workspace, kind.to_owned(), id))
				.cloned()
				.unwrap_or_default()
		}))
	}
	async fn registry_entries(&mut self, run: Uuid) -> Result<Vec<EntityRef>> {
		self.touch("registry")?;
		self.calls.push(format!("visited:{run}"));
		Ok(self.entries.clone())
	}
	async fn catalog_read(&mut self, _: Uuid, reference: &EntityRef) -> Result<()> {
		self.touch("catalog")?;
		self.calls
			.push(format!("catalog:{}:{}", reference.id, reference.version));
		if self.denied == Some("catalog") {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn remote_reads(&mut self, _: Uuid) -> Result<RemoteReads> {
		self.touch("remote")?;
		Ok(match self.denied {
			Some("remote") => RemoteReads::Unconfirmed,
			Some("tenant_agent") => RemoteReads::Definition(EntityRef {
				id: "sibling-agent".into(),
				version: "1.0.0".into(),
			}),
			_ => RemoteReads::Visible,
		})
	}
	async fn semantic_reads(&mut self, _: Uuid) -> Result<bool> {
		self.touch("semantic")?;
		Ok(self.denied != Some("semantic"))
	}
	async fn semantic_invalidity(
		&mut self,
		run: Uuid,
		pending: &mut Vec<Uuid>,
	) -> Result<Option<Failure>> {
		self.touch("semantic")?;
		if self
			.semantic_entry
			.as_ref()
			.is_some_and(|(root, _)| *root == run)
		{
			return crate::semantic::memory::reads_invalidity(self, run, pending).await;
		}
		Ok(self
			.invalid_runs
			.get(&run)
			.copied()
			.or_else(|| (self.denied == Some("semantic")).then_some(Failure::Authority)))
	}
	async fn received_semantic(&mut self, _: Uuid) -> Result<bool> {
		self.touch("received")?;
		Ok(self.denied != Some("received"))
	}
	async fn sources(&mut self, run: Uuid) -> Result<Vec<RecordedRead>> {
		self.touch("sources")?;
		Ok(self.graph.get(&run).cloned().unwrap_or_default())
	}
	async fn source_task(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Task>> {
		self.lookup("task", id, workspace)?;
		let mut t = task();
		t.id = id;
		t.workspace_id = workspace;
		Ok((!self.missing).then_some(t))
	}
	async fn source_artifact(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Artifact>> {
		self.lookup("artifact", id, workspace)?;
		let mut a = artifact();
		a.id = id;
		a.workspace_id = workspace;
		Ok((!self.missing).then_some(a))
	}
	async fn source_message(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Message>> {
		self.lookup("message", id, workspace)?;
		let mut m = message(id.as_u128(), "stored-sender");
		m.workspace_id = workspace;
		Ok((!self.missing).then_some(m))
	}
	async fn source_attachment_message(
		&mut self,
		id: Uuid,
		workspace: Uuid,
	) -> Result<Option<Message>> {
		self.lookup("attachment", id, workspace)?;
		let mut message = message(id.as_u128(), "stored-sender");
		message.workspace_id = workspace;
		Ok((!self.missing).then_some(message))
	}
	async fn source_run(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<RunMetadata>> {
		self.lookup("run", id, workspace)?;
		let mut r = run();
		r.id = id;
		r.workspace_id = workspace;
		Ok((!self.missing).then_some(r))
	}
	async fn source_conversation(
		&mut self,
		id: Uuid,
		workspace: Uuid,
	) -> Result<Option<Conversation>> {
		self.lookup("conversation", id, workspace)?;
		Ok((!self.missing).then(|| Conversation {
			scratch_space_id: uuid::Uuid::nil(),
			id,
			workspace_id: workspace,
			created_by: "stored-author".into(),
			target: "agent".into(),
			target_kind: "agent".into(),
			created_at: chrono::Utc::now(),
		}))
	}
	async fn source_generation(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Request>> {
		self.lookup("generation", id, workspace)?;
		let mut job = generation(id.as_u128());
		job.workspace_id = workspace;
		Ok((!self.missing).then_some(job))
	}
	async fn run_base_visible(&mut self, _: &RunMetadata) -> Result<bool> {
		self.touch("base")?;
		Ok(self.denied != Some("base"))
	}
	async fn classified_run_dependency_visible(
		&mut self,
		run: &RunMetadata,
		_: &mut Vec<Uuid>,
	) -> Result<bool> {
		self.run_base_visible(run).await
	}
	async fn generation_visible(&mut self, _: &Request) -> Result<bool> {
		self.touch("generation.read")?;
		Ok(self.denied != Some("generation.read"))
	}
}

#[async_trait]
impl crate::ports::semantic::memory::SemanticMemoryReadSession for Scope {
	async fn classified_output_source(
		&mut self,
		workspace: Uuid,
		source: &aidash_domain::semantic::Source,
		pending: &mut Vec<Uuid>,
	) -> Result<Option<String>> {
		classified_semantic_output_source(self, workspace, source, pending).await
	}
	async fn entry_authorized(
		&mut self,
		_: &aidash_domain::semantic::mutations::Entry,
	) -> Result<bool> {
		Ok(self.denied != Some("semantic.read"))
	}
	async fn permits(
		&mut self,
		entry: &aidash_domain::semantic::mutations::Entry,
		_: &str,
	) -> Result<bool> {
		let source: aidash_domain::semantic::Source = serde_json::from_value(entry.source.clone())?;
		let (kind, id) = match source {
			aidash_domain::semantic::Source::Artifact { id } => ("artifact", id),
			aidash_domain::semantic::Source::Message { id } => ("message", id),
			_ => unreachable!(),
		};
		ResourceVisibilityScope::output_visible(self, entry.workspace_id, kind, id).await
	}
	async fn source(
		&mut self,
		_: Uuid,
		_: &aidash_domain::semantic::Source,
	) -> Result<Option<String>> {
		Ok(Some("content".into()))
	}
	async fn dependencies(&mut self, run: Uuid) -> Result<Vec<(Uuid, i64)>> {
		Ok(self
			.semantic_entry
			.as_ref()
			.filter(|(root, _)| *root == run)
			.map(|(_, e)| vec![(e.id, e.revision)])
			.unwrap_or_default())
	}
	async fn entry(
		&mut self,
		id: Uuid,
	) -> Result<Option<aidash_domain::semantic::mutations::Entry>> {
		Ok(self
			.semantic_entry
			.as_ref()
			.filter(|(_, e)| e.id == id)
			.map(|(_, e)| e.clone()))
	}
	async fn point_digest(&mut self, _: Uuid) -> Result<Option<String>> {
		let kind = self.semantic_entry.as_ref().unwrap().1.source["kind"]
			.as_str()
			.unwrap();
		Ok(Some(aidash_domain::semantic::indexing::content_digest(
			if kind == "artifact" {
				"{}"
			} else {
				"same content"
			},
		)))
	}
}

fn semantic_output_fixture(mut scope: Scope, kind: &str) -> Scope {
	let root = Uuid::from_u128(1);
	let producer = Uuid::from_u128(2);
	let workspace = Uuid::from_u128(3);
	let source = Uuid::from_u128(4);
	scope
		.outputs
		.insert((workspace, kind.into(), source), vec![producer]);
	scope.semantic_entry = Some((
		root,
		aidash_domain::semantic::mutations::Entry {
			id: Uuid::from_u128(5),
			workspace_id: workspace,
			key: "key".into(),
			source: json!({"kind": kind, "id": source}),
			agent: None,
			metadata: json!({}),
			revision: 1,
			point_id: Uuid::from_u128(6),
			index_revision: 1,
			deleted: false,
			state: "READY".into(),
			attempts: 0,
			last_error: None,
			created_by: "author".into(),
			updated_at: chrono::Utc::now(),
		},
	));
	scope
}

#[rstest]
#[case("artifact")]
#[case("message")]
#[tokio::test]
async fn changed_semantic_source_producer_retains_invalidation(scope: Scope, #[case] kind: &str) {
	let mut scope = semantic_output_fixture(scope, kind);
	scope
		.invalid_runs
		.insert(Uuid::from_u128(2), Failure::Invalidated);
	assert_eq!(
		run_reads_invalidity(&mut scope, Uuid::from_u128(1))
			.await
			.unwrap(),
		Some(Failure::Invalidated)
	);
}

#[rstest]
#[case::cycle(None, None, None)]
#[case::configuration(Some(Failure::Configuration), None, Some(Failure::Configuration))]
#[case::semantic_authority(
	Some(Failure::Invalidated),
	Some("semantic.read"),
	Some(Failure::Authority)
)]
#[case::source_authority(
	Some(Failure::Invalidated),
	Some("message.read"),
	Some(Failure::Authority)
)]
#[case::output_authority(Some(Failure::Invalidated), Some("output"), Some(Failure::Authority))]
#[tokio::test]
async fn semantic_output_cycles_and_authority_keep_the_correct_cause(
	scope: Scope,
	#[case] producer_failure: Option<Failure>,
	#[case] denied: Option<&'static str>,
	#[case] expected: Option<Failure>,
) {
	let mut scope = semantic_output_fixture(scope, "message");
	let root = Uuid::from_u128(1);
	let producer = Uuid::from_u128(2);
	scope.outputs.insert(
		(Uuid::from_u128(3), "message".into(), Uuid::from_u128(4)),
		vec![root, producer],
	);
	if let Some(failure) = producer_failure {
		scope.invalid_runs.insert(producer, failure);
	}
	scope.denied = denied;
	assert_eq!(
		run_reads_invalidity(&mut scope, root).await.unwrap(),
		expected
	);
}

#[rstest]
#[case("task", "task")]
#[case("artifact", "artifact")]
#[case("artifact", "task")]
#[case("message", "message")]
#[tokio::test]
async fn changed_producer_source_retains_invalidation_through_output_provenance(
	mut scope: Scope,
	#[case] source_kind: &str,
	#[case] output_kind: &str,
) {
	let root = Uuid::from_u128(1);
	let producer = Uuid::from_u128(2);
	let workspace = Uuid::from_u128(3);
	let source = Uuid::from_u128(4);
	let output = if output_kind == "task" && source_kind == "artifact" {
		artifact().task_id
	} else {
		source
	};
	scope
		.graph
		.insert(root, vec![(workspace, source_kind.into(), source)]);
	scope
		.outputs
		.insert((workspace, output_kind.into(), output), vec![producer]);
	scope.invalid_runs.insert(producer, Failure::Invalidated);
	assert_eq!(
		run_reads_invalidity(&mut scope, root).await.unwrap(),
		Some(Failure::Invalidated)
	);
}

#[rstest]
#[tokio::test]
async fn output_producer_cycles_terminate_and_preserve_other_producer_failure(mut scope: Scope) {
	let root = Uuid::from_u128(1);
	let producer = Uuid::from_u128(2);
	let workspace = Uuid::from_u128(3);
	let message = Uuid::from_u128(4);
	scope
		.graph
		.insert(root, vec![(workspace, "message".into(), message)]);
	scope
		.graph
		.insert(producer, vec![(workspace, "message".into(), message)]);
	scope
		.outputs
		.insert((workspace, "message".into(), message), vec![root, producer]);
	assert_eq!(run_reads_invalidity(&mut scope, root).await.unwrap(), None);
	scope.invalid_runs.insert(producer, Failure::Configuration);
	assert_eq!(
		run_reads_invalidity(&mut scope, root).await.unwrap(),
		Some(Failure::Configuration)
	);
}

#[rstest]
#[case("message.read")]
#[case("output")]
#[tokio::test]
async fn denied_output_authority_is_not_masked_by_an_invalid_producer(
	mut scope: Scope,
	#[case] gate: &'static str,
) {
	let root = Uuid::from_u128(1);
	let producer = Uuid::from_u128(2);
	let workspace = Uuid::from_u128(3);
	let message = Uuid::from_u128(4);
	scope
		.graph
		.insert(root, vec![(workspace, "message".into(), message)]);
	scope
		.outputs
		.insert((workspace, "message".into(), message), vec![producer]);
	scope.invalid_runs.insert(producer, Failure::Invalidated);
	scope.denied = Some(gate);
	assert_eq!(
		run_reads_invalidity(&mut scope, root).await.unwrap(),
		Some(Failure::Authority)
	);
}

#[rstest]
#[case::deleted(true, None, Failure::Invalidated)]
#[case::revoked(false, Some("message.read"), Failure::Authority)]
#[tokio::test]
async fn memory_recheck_distinguishes_deleted_sources_from_lost_authority(
	mut scope: Scope,
	#[case] missing: bool,
	#[case] denied: Option<&'static str>,
	#[case] expected: Failure,
) {
	let run = Uuid::from_u128(1);
	scope.graph.insert(
		run,
		vec![(Uuid::from_u128(3), "message".into(), Uuid::from_u128(4))],
	);
	scope.missing = missing;
	scope.denied = denied;
	assert_eq!(
		run_reads_invalidity(&mut scope, run).await.unwrap(),
		Some(expected)
	);
}

#[rstest]
#[case("remote")]
#[case("received")]
#[tokio::test]
async fn opaque_remote_denials_keep_the_existing_invalidated_verdict(
	mut scope: Scope,
	#[case] gate: &'static str,
) {
	scope.denied = Some(gate);
	assert_eq!(
		run_reads_invalidity(&mut scope, Uuid::from_u128(1))
			.await
			.unwrap(),
		Some(Failure::Invalidated),
	);
}
#[rstest]
#[tokio::test]
async fn a_denied_agent_of_the_tenant_on_another_node_is_lost_authority(mut scope: Scope) {
	scope.denied = Some("tenant_agent");
	assert_eq!(
		run_reads_invalidity(&mut scope, Uuid::from_u128(1))
			.await
			.unwrap(),
		Some(Failure::Authority),
	);
}
#[rstest]
#[tokio::test]
async fn cycles_and_duplicate_run_references_terminate_after_checking_each_journal(
	mut scope: Scope,
) {
	let a = Uuid::from_u128(1);
	let b = Uuid::from_u128(2);
	let w = Uuid::from_u128(3);
	scope
		.graph
		.insert(a, vec![(w, "run".into(), b), (w, "run".into(), b)]);
	scope.graph.insert(
		b,
		vec![(w, "run".into(), a), (w, "task".into(), Uuid::from_u128(4))],
	);
	assert!(run_reads_visible(&mut scope, a).await.unwrap());
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|c| c.starts_with("visited:"))
			.cloned()
			.collect::<Vec<_>>(),
		vec![format!("visited:{a}"), format!("visited:{b}")]
	);
	assert!(scope.calls.contains(&"task.read".into()));
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|c| c.as_str() == "sources")
			.count(),
		2
	);
}
/// What a Run opens stays open only for that Run's own reads, however they end.
#[rstest]
#[tokio::test]
async fn a_run_opens_its_subnode_workspaces_only_while_its_own_reads_are_checked(
	mut scope: Scope,
	#[values(None, Some("sources"))] fail: Option<&'static str>,
) {
	let creator = Uuid::from_u128(1);
	let other = Uuid::from_u128(2);
	let workspace = Uuid::from_u128(3);
	scope.opening = vec![creator];
	scope
		.graph
		.insert(creator, vec![(workspace, "run".into(), other)]);
	scope
		.graph
		.insert(other, vec![(workspace, "task".into(), Uuid::from_u128(4))]);
	scope.fail = fail;
	let result = run_reads_visible(&mut scope, creator).await;
	assert_eq!(result.is_err(), fail.is_some());
	let position = |call: &str| scope.calls.iter().position(|c| c == call);
	let open = position(&format!("open:{creator}")).expect("the creator opens");
	let close = position("close").expect("and closes, however the check ended");
	assert!(open < close);
	assert_eq!(
		scope.calls.iter().filter(|c| c.as_str() == "close").count(),
		1
	);
	if fail.is_none() {
		// The other Run waits on the worklist until the creator's reads are done.
		assert!(close < position(&format!("visited:{other}")).unwrap());
	}
}
#[rstest]
#[tokio::test]
async fn the_recovery_walk_opens_a_runs_subnode_workspaces_the_same_way(mut scope: Scope) {
	let creator = Uuid::from_u128(1);
	scope.opening = vec![creator];
	scope.graph.insert(creator, vec![]);
	assert_eq!(
		run_reads_invalidity(&mut scope, creator).await.unwrap(),
		None
	);
	let open = format!("open:{creator}");
	assert!(scope.calls.contains(&open));
	assert_eq!(
		scope.calls.iter().filter(|c| c.as_str() == "close").count(),
		1
	);
}
#[rstest]
#[tokio::test]
async fn long_run_dependency_chains_use_a_worklist_instead_of_recursive_stack_frames(
	mut scope: Scope,
) {
	let workspace = Uuid::from_u128(9000);
	for id in 1..512 {
		scope.graph.insert(
			Uuid::from_u128(id),
			vec![(workspace, "run".into(), Uuid::from_u128(id + 1))],
		);
	}
	assert!(
		run_reads_visible(&mut scope, Uuid::from_u128(1))
			.await
			.unwrap()
	);
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|c| c.starts_with("visited:"))
			.count(),
		512
	);
}
#[rstest]
#[case("catalog")]
#[case("remote")]
#[case("semantic")]
#[case("received")]
#[tokio::test]
async fn hidden_dependency_journals_stop_before_loading_any_source_records(
	mut scope: Scope,
	#[case] gate: &'static str,
) {
	scope.denied = Some(gate);
	scope.entries = vec![EntityRef {
		id: "entry".into(),
		version: "1".into(),
	}];
	assert!(
		!run_reads_visible(&mut scope, Uuid::from_u128(1))
			.await
			.unwrap()
	);
	assert!(!scope.calls.contains(&"sources".into()));
}
#[rstest]
#[case("catalog", ReadBlock::Definition(EntityRef { id: "entry".into(), version: "1".into() }))]
#[case("remote", ReadBlock::RemoteRead)]
#[case("tenant_agent", ReadBlock::Definition(EntityRef { id: "sibling-agent".into(), version: "1.0.0".into() }))]
#[case("semantic", ReadBlock::SemanticRead)]
#[case("received", ReadBlock::SemanticRead)]
#[case("base", ReadBlock::Source("run".into()))]
#[tokio::test]
async fn a_walk_reports_the_read_that_stopped_it(
	mut scope: Scope,
	#[case] gate: &'static str,
	#[case] block: ReadBlock,
) {
	let root = Uuid::from_u128(1);
	scope.denied = Some(gate);
	scope.entries = vec![EntityRef {
		id: "entry".into(),
		version: "1".into(),
	}];
	scope.graph.insert(
		root,
		vec![(Uuid::from_u128(3), "run".into(), Uuid::from_u128(2))],
	);
	assert_eq!(
		run_reads_block(&mut scope, root).await.unwrap(),
		Some(block)
	);
	assert!(!run_reads_visible(&mut scope, root).await.unwrap());
}
#[rstest]
#[tokio::test]
async fn a_walk_that_passes_reports_no_block(mut scope: Scope) {
	assert_eq!(
		run_reads_block(&mut scope, Uuid::from_u128(1))
			.await
			.unwrap(),
		None
	);
}
#[rstest]
#[tokio::test]
async fn denied_nested_run_stops_before_reading_its_journal_or_later_sources(mut scope: Scope) {
	let root = Uuid::from_u128(1);
	scope.denied = Some("base");
	scope.graph.insert(
		root,
		vec![
			(Uuid::from_u128(3), "run".into(), Uuid::from_u128(2)),
			(Uuid::from_u128(3), "task".into(), Uuid::from_u128(5)),
		],
	);
	assert!(!run_reads_visible(&mut scope, root).await.unwrap());
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|c| c.starts_with("visited:"))
			.count(),
		1
	);
	assert!(!scope.calls.contains(&"task".into()));
}
#[rstest]
#[case("task")]
#[case("artifact")]
#[case("message")]
#[case("run")]
#[case("conversation")]
#[case("generation")]
#[tokio::test]
async fn source_lookups_retain_the_exact_journal_workspace_and_missing_rows_fail_closed(
	mut scope: Scope,
	#[case] kind: &str,
) {
	let workspace = Uuid::from_u128(3);
	let id = Uuid::from_u128(6);
	let mut pending = vec![];
	scope.missing = true;
	assert!(
		!source_visible(&mut scope, workspace, kind, id, &mut pending)
			.await
			.unwrap()
	);
	assert!(pending.is_empty());
	assert_eq!(
		scope.calls,
		vec![kind.to_owned(), format!("lookup:{kind}:{id}:{workspace}")]
	);
}
#[rstest]
#[case("workspace_events", "workspace.events")]
#[case("task", "task.read")]
#[case("artifact", "artifact.read")]
#[case("message", "message.read")]
#[case("run", "base")]
#[case("conversation", "conversation.read")]
#[case("generation", "generation.read")]
#[tokio::test]
async fn every_source_family_requires_its_current_read_policy(
	mut scope: Scope,
	#[case] kind: &str,
	#[case] gate: &'static str,
) {
	scope.denied = Some(gate);
	let mut pending = vec![];
	assert!(
		!source_visible(
			&mut scope,
			Uuid::from_u128(3),
			kind,
			Uuid::from_u128(6),
			&mut pending
		)
		.await
		.unwrap()
	);
	assert_eq!(
		pending,
		if kind == "run" {
			vec![Uuid::from_u128(6)]
		} else {
			vec![]
		}
	);
}
#[rstest]
#[case("unknown")]
#[case("")]
#[case("Task")]
#[tokio::test]
async fn unregistered_journal_source_kinds_fail_closed_without_queries(
	mut scope: Scope,
	#[case] kind: &str,
) {
	let mut pending = vec![];
	assert!(
		!source_visible(
			&mut scope,
			Uuid::from_u128(3),
			kind,
			Uuid::from_u128(6),
			&mut pending
		)
		.await
		.unwrap()
	);
	assert!(scope.calls.is_empty());
	assert!(pending.is_empty());
}
#[rstest]
#[tokio::test]
async fn successful_nested_run_is_enqueued_for_complete_provenance_checks(mut scope: Scope) {
	let mut pending = vec![Uuid::from_u128(99)];
	assert!(
		source_visible(
			&mut scope,
			Uuid::from_u128(3),
			"run",
			Uuid::from_u128(6),
			&mut pending
		)
		.await
		.unwrap()
	);
	assert_eq!(pending, vec![Uuid::from_u128(99), Uuid::from_u128(6)]);
}
#[rstest]
#[case("registry")]
#[case("catalog")]
#[case("remote")]
#[case("semantic")]
#[case("received")]
#[case("sources")]
#[tokio::test]
async fn journal_and_catalog_failures_are_preserved_without_becoming_denial_or_success(
	mut scope: Scope,
	#[case] boundary: &'static str,
) {
	scope.fail = Some(boundary);
	scope.entries = vec![EntityRef {
		id: "entry".into(),
		version: "1".into(),
	}];
	let error = run_reads_visible(&mut scope, Uuid::from_u128(1))
		.await
		.unwrap_err();
	assert!(matches!(error,Error::Port(ref e) if e.to_string()=="provenance adapter fault"));
	assert_eq!(scope.calls.last().map(String::as_str), Some(boundary));
}
#[rstest]
#[case("task")]
#[case("artifact")]
#[case("message")]
#[case("run")]
#[case("conversation")]
#[case("generation")]
#[case("base")]
#[case("generation.read")]
#[tokio::test]
async fn source_read_errors_stop_following_provenance_checks(
	mut scope: Scope,
	#[case] boundary: &'static str,
) {
	scope.fail = Some(boundary);
	let kind = match boundary {
		"base" => "run",
		"generation.read" => "generation",
		kind => kind,
	};
	let mut pending = vec![];
	let error = source_visible(
		&mut scope,
		Uuid::from_u128(3),
		kind,
		Uuid::from_u128(6),
		&mut pending,
	)
	.await
	.unwrap_err();
	assert!(matches!(error,Error::Port(ref e) if e.to_string()=="provenance adapter fault"));
	assert_eq!(scope.calls.last().map(String::as_str), Some(boundary));
}

#[fixture]
fn run() -> RunMetadata {
	RunMetadata {
		node: "brave-otter-k3m9q2xa".parse().unwrap(),
		id: Uuid::from_u128(1),
		task_id: Uuid::from_u128(2),
		workspace_id: Uuid::from_u128(3),
		home_node: "aidash://node".into(),
		agent_id: "agent".into(),
		agent_version: "1".into(),
		phase: aidash_domain::RunPhase::Ready,
		control: aidash_domain::RunControl::Active,
		step: 0,
		revision: 0,
		observed_input_seq: 0,
		ledger_worker_ready: false,
		control_revision: 0,
		control_source: Default::default(),
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: chrono::Utc::now(),
	}
}

fn generation(id: u128) -> Request {
	let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp(1000, 0).unwrap();
	aidash_domain::generation::requests::Request {
		node: "brave-otter-k3m9q2xa".parse().unwrap(),
		id: Uuid::from_u128(id),
		tenant: "tenant".into(),
		policy_id: "policy".into(),
		policy_revision: 7,
		task_id: Uuid::from_u128(2),
		home_node: String::new(),
		foreign_intent: None,
		prepared: true,
		grant_id: None,
		admission_id: None,
		workspace_id: Uuid::from_u128(3),
		credential_id: Uuid::from_u128(4),
		root_subject: "alice".into(),
		subject_chain: vec!["alice".into()],
		agent_id: "agent".into(),
		agent_version: "1.0.0".into(),
		definition: json!({}),
		status: "ACTIVE".into(),
		reason: "work".into(),
		depth: 1,
		token_limit: 10000,
		quota_released: false,
		expires_at: timestamp,
		created_at: timestamp,
	}
}

#[fixture]
fn task() -> Task {
	Task {
		id: Uuid::from_u128(1),
		workspace_id: Uuid::from_u128(2),
		title: "Task".into(),
		description: String::new(),
		status: aidash_domain::TaskStatus::Open,
		requirements: json!({}),
		owner: None,
		created_by: "stored-task-author".into(),
		dependencies: vec![],
		parent_id: None,
		revision: 0,
		created_at: chrono::Utc::now(),
	}
}

#[fixture]
fn artifact() -> Artifact {
	Artifact {
		id: Uuid::from_u128(3),
		workspace_id: Uuid::from_u128(2),
		task_id: Uuid::from_u128(1),
		kind: "text".into(),
		name: "Artifact".into(),
		content: json!({}),
		created_by: "stored-artifact-author".into(),
		idempotency_key: "key".into(),
		created_at: chrono::Utc::now(),
		run_id: None,
	}
}

fn message(id: u128, sender: &str) -> Message {
	Message {
		id: Uuid::from_u128(id),
		workspace_id: Uuid::from_u128(2),
		sender: sender.into(),
		content: "same content".into(),
		idempotency_key: None,
		created_at: chrono::Utc::now(),
		run_id: None,
	}
}

#[rstest]
#[case::visible(false, None, true)]
#[case::deleted(true, None, false)]
#[case::denied(false, Some("message.read"), false)]
#[tokio::test]
async fn attachment_provenance_requires_existing_attachment_and_readable_message(
	mut scope: Scope,
	#[case] missing: bool,
	#[case] denied: Option<&'static str>,
	#[case] expected: bool,
) {
	scope.missing = missing;
	scope.denied = denied;
	assert_eq!(
		source_visible(
			&mut scope,
			Uuid::from_u128(3),
			"attachment",
			Uuid::from_u128(4),
			&mut vec![]
		)
		.await
		.unwrap(),
		expected
	);
}
