use super::*;
use aidash_domain::{
	policy::Resource,
	registry::{
		Entry,
		workbench::{Draft, audit::Registration, inspection::TestObservation},
	},
};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone};
use rstest::rstest;
use serde_json::json;
use std::{collections::BTreeSet, sync::Mutex};
#[derive(Default)]
struct State {
	active: bool,
	finished: bool,
	cursors: Vec<Option<(DateTime<Utc>, Uuid)>>,
	titles: Vec<Uuid>,
	evidence_reads: usize,
	workspace_reads: usize,
}
struct Repository {
	principal: Principal,
	state: Mutex<State>,
	pages: Vec<Vec<RunMetadata>>,
	hidden: BTreeSet<Uuid>,
	forbidden: Option<Uuid>,
	failed: Option<Uuid>,
	inspection: bool,
	kind: &'static str,
	draft_allowed: bool,
	observations: usize,
	pause: bool,
}
impl Repository {
	fn new() -> Self {
		Self {
			principal: Principal::Subject {
				tenant: "tenant".into(),
				subject: "reader".into(),
			},
			state: Mutex::new(State::default()),
			pages: vec![],
			hidden: BTreeSet::new(),
			forbidden: None,
			failed: None,
			inspection: true,
			kind: "agent",
			draft_allowed: true,
			observations: 0,
			pause: false,
		}
	}
}
struct Scope<'a>(&'a Repository);
impl Drop for Scope<'_> {
	fn drop(&mut self) {
		self.0.state.lock().unwrap().active = false;
	}
}
fn reference() -> EntityRef {
	EntityRef {
		id: "agent".into(),
		version: "1".into(),
	}
}
fn at(n: i64) -> DateTime<Utc> {
	Utc.timestamp_opt(n, 0).unwrap()
}
fn run(n: u128, workspace: u128, phase: &str) -> RunMetadata {
	serde_json::from_value(json!({"id":Uuid::from_u128(n),"task_id":Uuid::from_u128(99),"workspace_id":Uuid::from_u128(workspace),"home_node":"node","agent_id":"agent","agent_version":"1","phase":phase,"control":"ACTIVE","step":0,"revision":1,"observed_input_seq":0,"ledger_worker_ready":true,"error":null,"lease_owner":null,"lease_until":null,"updated_at":at(2000-n as i64)})).unwrap()
}
#[async_trait]
impl InspectionRepository for Repository {
	fn node_id(&self) -> &str {
		"node"
	}
	async fn begin(&self) -> Result<Box<dyn InspectionScope + '_>> {
		let mut s = self.state.lock().unwrap();
		assert!(!s.active);
		s.active = true;
		Ok(Box::new(Scope(self)))
	}
}
#[async_trait]
impl InspectionScope for Scope<'_> {
	fn principal(&self) -> Principal {
		self.0.principal.clone()
	}
	async fn require_inspection(&mut self, entry: &EntityRef) -> Result<()> {
		assert_eq!(*entry, reference());
		if self.0.inspection {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
	async fn effective(&mut self, _: &EntityRef) -> Result<Entry> {
		Ok(serde_json::from_value(
			json!({"id":"agent","version":"1","kind":self.0.kind,"name":{},"description":{},"config":{}}),
		)?)
	}
	async fn agent_runs(
		&mut self,
		entry: &EntityRef,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<RunMetadata>> {
		assert_eq!(*entry, reference());
		let index = {
			let mut s = self.0.state.lock().unwrap();
			assert!(s.active);
			let index = s.cursors.len();
			s.cursors.push(cursor);
			index
		};
		if self.0.pause {
			std::future::pending::<()>().await;
		}
		Ok(self.0.pages.get(index).cloned().unwrap_or_default())
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		assert_ne!(self.principal(), Principal::Operator);
		let mut s = self.0.state.lock().unwrap();
		assert!(s.active);
		s.workspace_reads += 1;
		Ok(Resource {
			tenant: "tenant".into(),
			kind: "workspace".into(),
			id: id.to_string(),
			attributes: json!({}),
		})
	}
	async fn decide(&mut self, _: &Resource, action: &str) -> Result<bool> {
		assert_eq!(action, "workspace.read");
		Ok(true)
	}
	async fn run_visible(&mut self, row: &RunMetadata) -> Result<bool> {
		if self.0.failed == Some(row.id) {
			return Err(Error::External("authority store unavailable".into()));
		}
		if self.0.forbidden == Some(row.id) {
			return Err(Error::Forbidden);
		}
		Ok(!self.0.hidden.contains(&row.id))
	}
	async fn workspace_title(&mut self, id: Uuid) -> Result<Option<String>> {
		self.0.state.lock().unwrap().titles.push(id);
		Ok((id != Uuid::from_u128(3)).then(|| format!("workspace {id}")))
	}
	async fn registrations(&mut self, _: &EntityRef) -> Result<Vec<Registration>> {
		Ok(vec![Registration {
			draft_id: Uuid::from_u128(20),
			revision: 7,
			actor: "author".into(),
			registered_at: at(0),
		}])
	}
	async fn draft(&mut self, id: Uuid) -> Result<Draft> {
		Ok(Draft {
			id,
			tenant: "tenant".into(),
			owner: "author".into(),
			revision: 7,
			entry: json!({}),
			documents: json!([]),
			release_notes: String::new(),
			source_id: None,
			source_version: None,
			archived: false,
			updated_at: at(0),
		})
	}
	async fn authorize_draft(&mut self, draft: &Draft) -> Result<()> {
		assert_eq!(draft.id, Uuid::from_u128(20));
		if self.0.draft_allowed {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
	async fn test_observations(
		&mut self,
		draft: Uuid,
		revision: i64,
	) -> Result<Vec<TestObservation>> {
		assert!(self.0.draft_allowed);
		assert_eq!(draft, Uuid::from_u128(20));
		assert_eq!(revision, 7);
		self.0.state.lock().unwrap().evidence_reads += 1;
		Ok((0..self.0.observations)
			.map(|i| TestObservation {
				id: Uuid::from_u128(i as u128 + 100),
				status: "expired".into(),
				scenario: if i == 0 {
					json!({})
				} else {
					json!({"mode":"sandbox","profile_id":"profile","profile_revision":9})
				},
				usage: json!({"tokens":i}),
				created_at: at(10),
				expires_at: at(20),
				expired_at: Some(at(30)),
			})
			.collect())
	}
	async fn finish(self: Box<Self>, inspection: Inspection) -> Result<Inspection> {
		let mut s = self.0.state.lock().unwrap();
		assert!(s.active);
		s.finished = true;
		Ok(inspection)
	}
}
#[rstest]
#[case(false, "agent")]
#[case(true, "tool")]
#[tokio::test]
async fn inspection_authority_and_agent_kind_precede_usage(
	#[case] allowed: bool,
	#[case] kind: &'static str,
) {
	let mut r = Repository::new();
	r.inspection = allowed;
	r.kind = kind;
	let error = inspect(&r, reference()).await.unwrap_err();
	if !allowed {
		assert!(matches!(error, Error::Forbidden));
	} else {
		assert!(matches!(error,Error::NotFound(ref s) if s=="agent version"));
	}
	let s = r.state.lock().unwrap();
	assert!(!s.active && !s.finished);
	assert!(s.cursors.is_empty());
	assert_eq!(s.evidence_reads, 0);
}
#[rstest]
#[tokio::test]
async fn hidden_full_pages_do_not_consume_the_visible_usage_bound() {
	let mut r = Repository::new();
	r.pages = vec![
		(1..=501).map(|i| run(i, 1, "COMPLETED")).collect(),
		vec![run(502, 1, "COMPLETED"), run(503, 1, "THINKING")],
	];
	r.hidden = (1..=501).map(Uuid::from_u128).collect();
	let result = inspect(&r, reference()).await.unwrap();
	assert!(!result.usage_truncated);
	assert_eq!(result.workspaces.len(), 1);
	assert!(result.workspaces[0].current);
	assert_eq!(result.workspaces[0].latest_run_at, at(1498));
	assert_eq!(result.source_node, "node");
	assert!(!result.external_assessment_available);
	let s = r.state.lock().unwrap();
	assert_eq!(
		s.cursors,
		vec![None, Some((at(1499), Uuid::from_u128(501)))]
	);
	assert_eq!(s.titles, vec![Uuid::from_u128(1)]);
	assert!(!s.active && s.finished);
}
#[rstest]
#[tokio::test]
async fn overflow_counts_visible_runs_and_merges_current_and_latest_usage() {
	let mut r = Repository::new();
	r.pages = vec![
		(1..=501)
			.map(|i| {
				run(
					i,
					if i == 501 { 2 } else { 1 },
					if i == 2 { "WAITING" } else { "FAILED" },
				)
			})
			.collect(),
	];
	let result = inspect(&r, reference()).await.unwrap();
	assert!(result.usage_truncated);
	assert_eq!(result.workspaces.len(), 1);
	assert!(result.workspaces[0].current);
	assert_eq!(result.workspaces[0].latest_run_at, at(1999));
	assert_eq!(r.state.lock().unwrap().titles, vec![Uuid::from_u128(1)]);
}
#[rstest]
#[tokio::test]
async fn operator_uses_existing_inspection_authority_without_subject_resource_checks() {
	let mut r = Repository::new();
	r.principal = Principal::Operator;
	r.forbidden = Some(Uuid::from_u128(1));
	r.pages = vec![vec![
		run(1, 1, "READY"),
		run(2, 2, "CANCELLED"),
		run(3, 3, "COMPLETED"),
	]];
	let result = inspect(&r, reference()).await.unwrap();
	assert_eq!(
		result
			.workspaces
			.iter()
			.map(|w| w.workspace_id)
			.collect::<Vec<_>>(),
		vec![Uuid::from_u128(1), Uuid::from_u128(2)]
	);
	assert!(result.workspaces[0].current);
	assert!(!result.workspaces[1].current);
	assert_eq!(r.state.lock().unwrap().workspace_reads, 0);
}
#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn forbidden_runs_are_hidden_but_authority_store_failures_are_not(#[case] external: bool) {
	let mut r = Repository::new();
	r.pages = vec![vec![run(1, 1, "READY")]];
	if external {
		r.failed = Some(Uuid::from_u128(1));
	} else {
		r.forbidden = Some(Uuid::from_u128(1));
	}
	let result = inspect(&r, reference()).await;
	if external {
		assert!(matches!(result, Err(Error::External(_))));
		assert!(!r.state.lock().unwrap().finished);
	} else {
		assert!(result.unwrap().workspaces.is_empty());
	}
	assert!(!r.state.lock().unwrap().active);
}
#[rstest]
#[case(false, 101, 0, false)]
#[case(true, 100, 100, false)]
#[case(true, 101, 100, true)]
#[tokio::test]
async fn sandbox_evidence_requires_current_draft_sharing_and_preserves_overflow_and_expiry(
	#[case] allowed: bool,
	#[case] count: usize,
	#[case] expected: usize,
	#[case] truncated: bool,
) {
	let mut r = Repository::new();
	r.draft_allowed = allowed;
	r.observations = count;
	let result = inspect(&r, reference()).await.unwrap();
	assert_eq!(result.test_evidence.len(), expected);
	assert_eq!(result.test_evidence_truncated, truncated);
	assert_eq!(r.state.lock().unwrap().evidence_reads, usize::from(allowed));
	if allowed {
		assert_eq!(result.test_evidence[0].mode, "unknown");
		assert_eq!(result.test_evidence[0].profile_id, None);
		assert_eq!(
			result.test_evidence[1].profile_id.as_deref(),
			Some("profile")
		);
		assert_eq!(result.test_evidence[1].profile_revision, Some(9));
		for row in &result.test_evidence {
			assert_eq!(row.draft_revision, 7);
			assert_eq!(row.status, "expired");
			assert_eq!(row.expired_at, Some(at(30)));
		}
	}
}
#[rstest]
#[tokio::test]
async fn cancellation_releases_the_same_audit_allocation_without_finishing() {
	let mut r = Repository::new();
	r.pause = true;
	let mut future = Box::pin(inspect(&r, reference()));
	assert!(futures_util::poll!(&mut future).is_pending());
	assert!(r.state.lock().unwrap().active);
	drop(future);
	let s = r.state.lock().unwrap();
	assert!(!s.active && !s.finished);
}
