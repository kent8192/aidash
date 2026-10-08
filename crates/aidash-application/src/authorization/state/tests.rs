use super::*;
use crate::Error;
use aidash_domain::{
	RunControl, RunMetadata, RunPhase, TaskStatus, context::Context, policy::Resource,
	run_state::RawRun,
};
use async_trait::async_trait;
use chrono::Utc;
use rstest::{fixture, rstest};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
fn id(value: u128) -> Uuid {
	Uuid::from_u128(value)
}
fn workspace() -> Workspace {
	Workspace {
		id: id(1),
		title: "saved workspace".into(),
		goal: "goal".into(),
		state: json!({}),
		revision: 4,
		created_at: Utc::now(),
	}
}
fn task(value: u128) -> Task {
	Task {
		id: id(value),
		workspace_id: id(1),
		title: "task".into(),
		description: "description".into(),
		status: TaskStatus::Open,
		requirements: json!({}),
		owner: None,
		created_by: "saved-author".into(),
		dependencies: vec![],
		parent_id: None,
		revision: 1,
		created_at: Utc::now(),
	}
}
fn artifact(value: u128, visible: bool) -> Artifact {
	Artifact {
		id: id(value),
		workspace_id: id(1),
		task_id: id(2),
		kind: "text".into(),
		name: "artifact".into(),
		content: json!({"saved":true}),
		created_by: if visible { "saved-author" } else { "hidden" }.into(),
		idempotency_key: "immutable".into(),
		created_at: Utc::now(),
	}
}
fn run(value: u128, visible: bool) -> RawRun {
	RawRun {
		metadata: RunMetadata {
			id: id(value),
			task_id: id(2),
			workspace_id: id(1),
			home_node: "aidash://home".into(),
			agent_id: if visible { "saved-agent" } else { "hidden" }.into(),
			agent_version: "1".into(),
			phase: RunPhase::Ready,
			control: RunControl::Active,
			step: 0,
			revision: 3,
			observed_input_seq: 0,
			ledger_worker_ready: false,
			error: None,
			lease_owner: None,
			lease_until: None,
			updated_at: Utc::now(),
		},
		context: serde_json::to_value(Context::default()).unwrap(),
		pending: json!({"state_version":1,"data":{},"recovery":{"retry":null,"lease_recovered":false}}),
	}
}
fn human(value: u128, visible: bool) -> HumanRequest {
	HumanRequest {
		id: id(value),
		workspace_id: id(1),
		run_id: id(3),
		kind: "question".into(),
		prompt: if visible { "saved-author" } else { "hidden" }.into(),
		response: None,
		answered_by: None,
		created_at: Utc::now(),
	}
}
fn conversation(value: u128, visible: bool) -> Conversation {
	Conversation {
		id: id(value),
		workspace_id: id(1),
		target: "saved-target".into(),
		target_kind: "human".into(),
		created_by: if visible { "saved-author" } else { "hidden" }.into(),
		created_at: Utc::now(),
	}
}
fn entry() -> Entry {
	Entry {
		binding_normalization: None,
		id: "saved-agent".into(),
		version: "1".into(),
		kind: "agent".into(),
		name: BTreeMap::new(),
		description: BTreeMap::new(),
		capabilities: vec![],
		tags: vec![],
		languages: vec![],
		skills: vec![],
		schema: json!({}),
		config: json!({}),
		installation: None,
	}
}
#[derive(Default)]
struct Scope {
	selected: Vec<Uuid>,
	events_allowed: BTreeSet<Uuid>,
	tasks: Vec<Task>,
	next_offset: Option<u64>,
	artifacts: BTreeMap<u64, Vec<Artifact>>,
	runs: BTreeMap<i64, Vec<RawRun>>,
	humans: BTreeMap<i64, Vec<HumanRequest>>,
	conversations: BTreeMap<i64, Vec<Conversation>>,
	calls: Vec<&'static str>,
	cursors: Vec<(&'static str, i64)>,
	event_scopes: Vec<(Vec<Uuid>, bool)>,
	human_scopes: Vec<Vec<Uuid>>,
	decisions: Vec<(Resource, String)>,
	visited: Vec<(&'static str, Uuid)>,
	fail: Option<&'static str>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		selected: vec![id(1), id(2)],
		events_allowed: BTreeSet::from([id(1)]),
		tasks: vec![task(2)],
		next_offset: Some(9),
		artifacts: BTreeMap::from([(0, vec![artifact(4, true)])]),
		runs: BTreeMap::from([(0, vec![run(3, true)])]),
		humans: BTreeMap::from([(0, vec![human(5, true)])]),
		conversations: BTreeMap::from([(0, vec![conversation(6, true)])]),
		..Scope::default()
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
	fn resource(&self, kind: &str, row: Uuid, saved: &str) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: kind.into(),
			id: row.to_string(),
			attributes: json!({"saved":saved}),
		}
	}
	fn fill(&mut self, kind: &str, offset: i64, count: usize, visible: bool) {
		let start = offset as u128 + 100;
		match kind {
			"artifact" => {
				self.artifacts.insert(
					offset as u64,
					(0..count)
						.map(|n| artifact(start + n as u128, visible))
						.collect(),
				);
			}
			"run" => {
				self.runs.insert(
					offset,
					(0..count)
						.map(|n| run(start + n as u128, visible))
						.collect(),
				);
			}
			"human" => {
				self.humans.insert(
					offset,
					(0..count)
						.map(|n| human(start + n as u128, visible))
						.collect(),
				);
			}
			"conversation" => {
				self.conversations.insert(
					offset,
					(0..count)
						.map(|n| conversation(start + n as u128, visible))
						.collect(),
				);
			}
			_ => panic!("unknown collection"),
		}
	}
}
#[async_trait]
impl WorkspaceListingScope for Scope {
	async fn visible(&mut self, action: &str) -> Result<Vec<Uuid>> {
		assert_eq!(action, "workspace.read");
		self.touch("visible")?;
		Ok(self.selected.clone())
	}
	async fn task_page(&mut self, workspaces: &[Uuid], offset: u64) -> Result<TaskPage> {
		assert_eq!(workspaces, self.selected);
		self.touch("tasks")?;
		self.cursors.push(("task", offset as i64));
		Ok(TaskPage {
			tasks: self.tasks.clone(),
			next_offset: self.next_offset,
		})
	}
}
#[async_trait]
impl WorkspaceStateScope for Scope {
	async fn allowed(&mut self, workspace: Uuid, action: &str) -> Result<bool> {
		assert_eq!(action, "workspace.events");
		self.touch("allowed")?;
		assert!(self.selected.contains(&workspace));
		Ok(self.events_allowed.contains(&workspace))
	}
	async fn registry(&mut self) -> Result<Vec<Entry>> {
		self.touch("registry")?;
		Ok(vec![entry()])
	}
	async fn workspace_rows(&mut self, workspaces: &[Uuid]) -> Result<Vec<Workspace>> {
		assert_eq!(workspaces, self.selected);
		self.touch("workspaces")?;
		Ok(workspaces
			.iter()
			.map(|selected| Workspace {
				id: *selected,
				..workspace()
			})
			.collect())
	}
	async fn latest_visible_events(
		&mut self,
		workspaces: &[Uuid],
		include_marketplace: bool,
	) -> Result<Vec<Event>> {
		self.touch("events")?;
		self.event_scopes
			.push((workspaces.to_vec(), include_marketplace));
		Ok(vec![Event {
			id: id(7),
			sequence: 9,
			node_id: "aidash://home".into(),
			workspace_id: None,
			kind: "marketplace.installed".into(),
			data: json!({"saved":true}),
			created_at: Utc::now(),
		}])
	}
	async fn artifact_rows(&mut self, workspaces: &[Uuid], offset: u64) -> Result<Vec<Artifact>> {
		assert_eq!(workspaces, self.selected);
		self.touch("artifacts")?;
		self.cursors.push(("artifact", offset as i64));
		Ok(self.artifacts.get(&offset).cloned().unwrap_or_default())
	}
	async fn raw_runs(&mut self, workspaces: &[Uuid], offset: i64) -> Result<Vec<RawRun>> {
		assert_eq!(workspaces, self.selected);
		self.touch("runs")?;
		self.cursors.push(("run", offset));
		Ok(self.runs.get(&offset).cloned().unwrap_or_default())
	}
	async fn human_rows(&mut self, runs: &[Uuid], offset: i64) -> Result<Vec<HumanRequest>> {
		self.touch("humans")?;
		self.human_scopes.push(runs.to_vec());
		self.cursors.push(("human", offset));
		Ok(self
			.humans
			.get(&offset)
			.cloned()
			.unwrap_or_default()
			.into_iter()
			.filter(|row| runs.contains(&row.run_id))
			.collect())
	}
	async fn conversation_rows(
		&mut self,
		workspaces: &[Uuid],
		offset: i64,
	) -> Result<Vec<Conversation>> {
		assert_eq!(workspaces, self.selected);
		self.touch("conversations")?;
		self.cursors.push(("conversation", offset));
		Ok(self.conversations.get(&offset).cloned().unwrap_or_default())
	}
	async fn artifact_visible(&mut self, row: &Artifact) -> Result<bool> {
		self.touch("artifact_visible")?;
		self.visited.push(("artifact", row.id));
		Ok(row.created_by != "hidden")
	}
	async fn run_visible(&mut self, row: &RunMetadata) -> Result<bool> {
		self.touch("run_visible")?;
		self.visited.push(("run", row.id));
		Ok(row.agent_id != "hidden")
	}
	async fn human_resource(&mut self, row: &HumanRequest) -> Result<Resource> {
		self.touch("human_resource")?;
		self.visited.push(("human", row.id));
		Ok(self.resource("human", row.id, &row.prompt))
	}
	async fn conversation_resource(&mut self, row: &Conversation) -> Result<Resource> {
		self.touch("conversation_resource")?;
		self.visited.push(("conversation", row.id));
		Ok(self.resource("conversation", row.id, &row.created_by))
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		assert_eq!(action, format!("{}.read", resource.kind));
		self.touch("decide")?;
		self.decisions.push((resource.clone(), action.into()));
		Ok(resource.attributes["saved"] != "hidden")
	}
}
fn disclosed_ids(state: &WorkspaceState, kind: &str) -> Vec<Uuid> {
	match kind {
		"artifact" => state.artifacts.iter().map(|row| row.id).collect(),
		"run" => state.runs.iter().map(|row| row.id).collect(),
		"human" => state.human_requests.iter().map(|row| row.id).collect(),
		"conversation" => state.conversations.iter().map(|row| row.id).collect(),
		_ => panic!("unknown collection"),
	}
}
#[rstest]
#[tokio::test]
async fn current_state_keeps_its_records_order_and_saved_authority(mut scope: Scope) {
	let output = state(&mut scope).await.unwrap();
	assert_eq!(output.registry, vec![entry()]);
	assert_eq!(
		output
			.workspaces
			.iter()
			.map(|row| row.id)
			.collect::<Vec<_>>(),
		vec![id(1), id(2)]
	);
	assert_eq!(output.tasks[0].id, id(2));
	assert_eq!(output.artifacts[0].created_by, "saved-author");
	assert_eq!(output.runs[0].id, id(3));
	assert_eq!(output.runs[0].state_error, None);
	assert_eq!(output.human_requests[0].id, id(5));
	assert_eq!(output.conversations[0].created_by, "saved-author");
	assert_eq!(output.events[0].sequence, 9);
	assert_eq!(scope.event_scopes, vec![(vec![id(1)], true)]);
	assert_eq!(scope.human_scopes, vec![vec![id(3)]]);
	assert_eq!(
		scope
			.decisions
			.iter()
			.map(|(row, action)| (row.id.clone(), action.clone()))
			.collect::<Vec<_>>(),
		vec![
			(id(5).to_string(), "human.read".into()),
			(id(6).to_string(), "conversation.read".into())
		]
	);
	assert_eq!(
		scope.calls,
		vec![
			"visible",
			"allowed",
			"allowed",
			"registry",
			"workspaces",
			"tasks",
			"events",
			"artifacts",
			"artifact_visible",
			"runs",
			"run_visible",
			"humans",
			"human_resource",
			"decide",
			"conversations",
			"conversation_resource",
			"decide"
		]
	);
}
#[rstest]
#[tokio::test]
async fn denied_workspace_events_do_not_remove_tenant_marketplace_events(mut scope: Scope) {
	scope.events_allowed.clear();
	let output = state(&mut scope).await.unwrap();
	assert_eq!(scope.event_scopes, vec![(vec![], true)]);
	assert_eq!(output.events[0].workspace_id, None);
	assert_eq!(output.events[0].kind, "marketplace.installed");
}
#[rstest]
#[tokio::test]
async fn no_visible_workspace_still_loads_the_current_catalog_and_empty_scoped_pages(
	mut scope: Scope,
) {
	scope.selected.clear();
	scope.tasks.clear();
	scope.artifacts.clear();
	scope.runs.clear();
	scope.humans.clear();
	scope.conversations.clear();
	let output = state(&mut scope).await.unwrap();
	assert_eq!(output.registry, vec![entry()]);
	assert!(output.workspaces.is_empty());
	assert!(output.tasks.is_empty());
	assert!(output.runs.is_empty());
	assert!(scope.calls.contains(&"registry"));
	assert_eq!(scope.human_scopes, vec![Vec::<Uuid>::new()]);
	assert_eq!(scope.event_scopes, vec![(vec![], true)]);
	assert!(!scope.calls.contains(&"allowed"));
}
#[rstest]
#[case::context(true)]
#[case::pending(false)]
#[tokio::test]
async fn a_malformed_visible_run_is_inspectable_without_poisoning_other_rows(
	mut scope: Scope,
	#[case] corrupt_context: bool,
) {
	let mut malformed = run(8, true);
	if corrupt_context {
		malformed.context = json!({"incomplete":true});
	} else {
		malformed.pending = json!({"state_version":999});
	}
	assert!(malformed.decode().is_err());
	scope.runs.insert(0, vec![malformed, run(3, true)]);
	let output = state(&mut scope).await.unwrap();
	assert_eq!(
		output.runs.iter().map(|row| row.id).collect::<Vec<_>>(),
		vec![id(8), id(3)]
	);
	assert!(output.runs[0].state_error.is_some());
	assert!(output.runs[0].context.is_none());
	assert_eq!(output.runs[1].state_error, None);
	assert!(output.runs[1].context.is_some());
	assert_eq!(scope.human_scopes, vec![vec![id(8), id(3)]]);
}
#[rstest]
#[tokio::test]
async fn a_hidden_malformed_run_never_enters_inspection_or_human_request_scope(mut scope: Scope) {
	let mut hidden = run(8, false);
	hidden.pending = json!({});
	hidden.context = json!([]);
	scope.runs.insert(0, vec![hidden, run(3, true)]);
	let output = state(&mut scope).await.unwrap();
	assert_eq!(output.runs.len(), 1);
	assert_eq!(output.runs[0].id, id(3));
	assert_eq!(output.runs[0].state_error, None);
	assert_eq!(scope.human_scopes, vec![vec![id(3)]]);
	assert!(scope.visited.contains(&("run", id(8))));
}
#[rstest]
#[case::artifacts("artifact")]
#[case::runs("run")]
#[case::humans("human")]
#[case::conversations("conversation")]
#[tokio::test]
async fn full_hidden_pages_advance_the_candidate_offset_before_a_visible_record(
	mut scope: Scope,
	#[case] kind: &str,
) {
	scope.fill(kind, 0, 500, false);
	scope.fill(kind, 500, 1, true);
	let output = state(&mut scope).await.unwrap();
	assert_eq!(disclosed_ids(&output, kind), vec![id(600)]);
	assert_eq!(
		scope
			.cursors
			.iter()
			.filter(|(collection, _)| *collection == kind)
			.map(|(_, offset)| *offset)
			.collect::<Vec<_>>(),
		vec![0, 500]
	);
	assert_eq!(
		scope
			.visited
			.iter()
			.filter(|(collection, _)| *collection == kind)
			.count(),
		501
	);
}
#[rstest]
#[case::artifacts("artifact")]
#[case::runs("run")]
#[case::humans("human")]
#[case::conversations("conversation")]
#[tokio::test]
async fn the_limit_counts_visible_rows_and_stops_before_unused_candidates(
	mut scope: Scope,
	#[case] kind: &str,
) {
	scope.fill(kind, 0, 498, true);
	scope.fill(kind, 500, 3, true);
	match kind {
		"artifact" => scope
			.artifacts
			.get_mut(&0)
			.unwrap()
			.extend([artifact(598, false), artifact(599, false)]),
		"run" => scope
			.runs
			.get_mut(&0)
			.unwrap()
			.extend([run(598, false), run(599, false)]),
		"human" => scope
			.humans
			.get_mut(&0)
			.unwrap()
			.extend([human(598, false), human(599, false)]),
		"conversation" => scope
			.conversations
			.get_mut(&0)
			.unwrap()
			.extend([conversation(598, false), conversation(599, false)]),
		_ => panic!("unknown collection"),
	};
	let output = state(&mut scope).await.unwrap();
	let rows = disclosed_ids(&output, kind);
	assert_eq!(rows.len(), 500);
	assert_eq!(rows[497], id(597));
	assert_eq!(rows[498], id(600));
	assert_eq!(rows[499], id(601));
	assert!(!scope.visited.contains(&(kind, id(602))));
	assert_eq!(
		scope
			.cursors
			.iter()
			.filter(|(collection, _)| *collection == kind)
			.map(|(_, offset)| *offset)
			.collect::<Vec<_>>(),
		vec![0, 500]
	);
}
#[rstest]
#[case::artifacts("artifact")]
#[case::runs("run")]
#[case::humans("human")]
#[case::conversations("conversation")]
#[tokio::test]
async fn an_exactly_full_visible_page_does_not_fetch_a_second_page(
	mut scope: Scope,
	#[case] kind: &str,
) {
	scope.fill(kind, 0, 500, true);
	let output = state(&mut scope).await.unwrap();
	assert_eq!(disclosed_ids(&output, kind).len(), 500);
	assert_eq!(
		scope
			.cursors
			.iter()
			.filter(|(collection, _)| *collection == kind)
			.map(|(_, offset)| *offset)
			.collect::<Vec<_>>(),
		vec![0]
	);
}
#[rstest]
#[case::artifacts("artifact")]
#[case::runs("run")]
#[case::humans("human")]
#[case::conversations("conversation")]
#[tokio::test]
async fn hidden_rows_never_exhaust_the_visible_limit(mut scope: Scope, #[case] kind: &str) {
	scope.fill(kind, 0, 500, false);
	let output = state(&mut scope).await.unwrap();
	assert!(disclosed_ids(&output, kind).is_empty());
	assert_eq!(
		scope
			.cursors
			.iter()
			.filter(|(collection, _)| *collection == kind)
			.map(|(_, offset)| *offset)
			.collect::<Vec<_>>(),
		vec![0, 500]
	);
}
#[rstest]
#[case::visible("visible")]
#[case::event_authority("allowed")]
#[case::registry("registry")]
#[case::workspace_rows("workspaces")]
#[case::tasks("tasks")]
#[case::events("events")]
#[case::artifact_rows("artifacts")]
#[case::artifact_authority("artifact_visible")]
#[case::raw_runs("runs")]
#[case::run_authority("run_visible")]
#[case::human_rows("humans")]
#[case::human_resource("human_resource")]
#[case::human_policy("decide")]
#[case::conversation_rows("conversations")]
#[case::conversation_resource("conversation_resource")]
#[tokio::test]
async fn partial_state_is_never_returned_when_a_read_or_policy_fails(
	mut scope: Scope,
	#[case] fail: &'static str,
) {
	scope.fail = Some(fail);
	let Error::Port(error) = state(&mut scope).await.err().unwrap() else {
		panic!("expected opaque fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		fail
	);
	assert_eq!(scope.calls.last(), Some(&fail));
}
#[rstest]
#[tokio::test]
async fn standalone_task_pages_use_current_workspace_visibility_and_keep_the_storage_cursor(
	mut scope: Scope,
) {
	let page = task_page(&mut scope, 400).await.unwrap();
	assert_eq!(page.tasks[0].id, id(2));
	assert_eq!(page.next_offset, Some(9));
	assert_eq!(scope.calls, vec!["visible", "tasks"]);
	assert_eq!(scope.cursors, vec![("task", 400)]);
}
#[rstest]
#[tokio::test]
async fn task_page_authority_failure_precedes_any_task_query(mut scope: Scope) {
	scope.fail = Some("visible");
	assert!(matches!(
		task_page(&mut scope, 0).await,
		Err(Error::Port(_))
	));
	assert_eq!(scope.calls, vec!["visible"]);
	assert!(scope.cursors.is_empty());
}

#[rstest]
#[tokio::test]
async fn conversation_policy_failure_does_not_return_the_already_built_projection(
	mut scope: Scope,
) {
	scope.humans.clear();
	scope.fail = Some("decide");
	let Error::Port(error) = state(&mut scope).await.err().unwrap() else {
		panic!("expected policy fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		"decide"
	);
	assert_eq!(scope.calls.last(), Some(&"decide"));
	assert_eq!(scope.visited.last(), Some(&("conversation", id(6))));
}
