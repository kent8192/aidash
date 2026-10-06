use super::*;
use crate::ports::registry::workbench::incidents::IncidentRetentionScope;
use aidash_domain::{policy::Decision, registry::Entry};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone};
use rstest::rstest;
use serde_json::Value;
use std::sync::Mutex;
#[derive(Default)]
struct State {
	active: bool,
	commits: usize,
	calls: Vec<&'static str>,
	evaluations: Vec<Evaluation>,
	cursors: Vec<Option<(DateTime<Utc>, Uuid)>>,
	saved: Option<Incident>,
	events: Vec<(String, Value)>,
	discarded: Vec<(Uuid, Value)>,
}
struct Repository {
	principal: Principal,
	state: Mutex<State>,
	prior: Incident,
	pages: Vec<Vec<Incident>>,
	allowed: bool,
	kind: &'static str,
	event_failure: bool,
	pause_event: bool,
	discard_failure: bool,
}
impl Repository {
	fn new() -> Self {
		Self {
			principal: Principal::Subject {
				tenant: "tenant".into(),
				subject: "reader".into(),
			},
			state: Mutex::new(State::default()),
			prior: incident(1),
			pages: vec![],
			allowed: true,
			kind: "agent",
			event_failure: false,
			pause_event: false,
			discard_failure: false,
		}
	}
}
struct Scope<'a> {
	repository: &'a Repository,
	saved: Option<Incident>,
	events: Vec<(String, Value)>,
	discarded: Vec<(Uuid, Value)>,
}
impl Drop for Scope<'_> {
	fn drop(&mut self) {
		self.repository.state.lock().unwrap().active = false;
	}
}
impl Scope<'_> {
	fn call(&self, call: &'static str) {
		let mut s = self.repository.state.lock().unwrap();
		assert!(s.active);
		s.calls.push(call);
	}
	fn complete(self: Box<Self>) -> Result<()> {
		let mut s = self.repository.state.lock().unwrap();
		assert!(s.active);
		s.commits += 1;
		if let Some(saved) = &self.saved {
			s.saved = Some(saved.clone());
		}
		s.events.extend(self.events.clone());
		s.discarded.extend(self.discarded.clone());
		Ok(())
	}
}
fn at(n: i64) -> DateTime<Utc> {
	Utc.timestamp_opt(n, 0).unwrap()
}
fn reference() -> EntityRef {
	EntityRef {
		id: "agent".into(),
		version: "1".into(),
	}
}
fn incident(id: u128) -> Incident {
	Incident {
		id: Uuid::from_u128(id),
		tenant: "tenant".into(),
		agent_id: "agent".into(),
		version: "1".into(),
		revision: 5,
		severity: "low".into(),
		status: "open".into(),
		archived: false,
		owner: "owner".into(),
		notes: "source notes".into(),
		evidence: json!([]),
		created_at: at(1000 - id as i64),
		updated_at: at(1000),
		resolved_at: None,
		evidence_expires_at: None,
		evidence_expired_at: None,
	}
}
fn new_incident() -> CreateIncident {
	CreateIncident {
		tenant: None,
		severity: "high".into(),
		owner: "owner".into(),
		notes: "report".into(),
		evidence: vec![EvidenceInput {
			title: "source".into(),
			content: "hello world".into(),
		}],
	}
}
fn change(status: &str) -> UpdateIncident {
	UpdateIncident {
		expected_revision: 5,
		severity: "medium".into(),
		status: status.into(),
		archived: None,
		owner: "next".into(),
		notes: "changed".into(),
		add_evidence: vec![],
	}
}
#[async_trait]
impl IncidentRepository for Repository {
	fn principal(&self) -> Principal {
		self.principal.clone()
	}
	async fn begin(&self) -> Result<Box<dyn IncidentScope + '_>> {
		let mut s = self.state.lock().unwrap();
		assert!(!s.active);
		s.active = true;
		s.calls.push("begin");
		Ok(Box::new(Scope {
			repository: self,
			saved: None,
			events: vec![],
			discarded: vec![],
		}))
	}
}
#[async_trait]
impl IncidentScope for Scope<'_> {
	fn principal(&self) -> Principal {
		self.repository.principal.clone()
	}
	async fn lock_identity(&mut self) -> Result<()> {
		self.call("identity");
		Ok(())
	}
	async fn evaluate(&mut self, tenant: &str, e: &Evaluation) -> Result<Decision> {
		assert_eq!(tenant, "tenant");
		assert_eq!(e.subject, "reader");
		assert_eq!(e.resource.kind, "agent_incident");
		assert_eq!(e.resource.attributes["agent_id"], "agent");
		assert_eq!(e.resource.attributes["version"], "1");
		assert_eq!(e.environment, json!({}));
		self.call("policy");
		self.repository
			.state
			.lock()
			.unwrap()
			.evaluations
			.push(e.clone());
		Ok(Decision {
			allowed: self.repository.allowed && e.resource.id != Uuid::from_u128(2).to_string(),
			reason: "fixture".into(),
			revision: 7,
			matched_policies: vec![],
			effective_roles: Default::default(),
		})
	}
	async fn require_inspection(&mut self, entry: &EntityRef) -> Result<()> {
		assert_eq!(*entry, reference());
		self.call("inspection");
		Ok(())
	}
	async fn effective(&mut self, _: &EntityRef) -> Result<Entry> {
		self.call("effective");
		Ok(serde_json::from_value(
			json!({"id":"agent","version":"1","kind":self.repository.kind,"name":{},"description":{},"config":{}}),
		)?)
	}
	async fn target_enabled(&mut self, tenant: &str, subject: &str) -> Result<()> {
		assert_eq!(tenant, "tenant");
		assert!(matches!(subject, "owner" | "next"));
		self.call("target");
		Ok(())
	}
	async fn read(&mut self, id: Uuid, lock: bool) -> Result<Incident> {
		self.call(if lock { "read_locked" } else { "read" });
		let row = self
			.saved
			.as_ref()
			.unwrap_or(&self.repository.prior)
			.clone();
		assert_eq!(row.id, id);
		Ok(row)
	}
	async fn page(
		&mut self,
		entry: &EntityRef,
		tenant: Option<&str>,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<Incident>> {
		assert_eq!(*entry, reference());
		assert_eq!(tenant, Some("tenant"));
		let mut s = self.repository.state.lock().unwrap();
		let index = s.cursors.len();
		s.cursors.push(cursor);
		Ok(self
			.repository
			.pages
			.get(index)
			.cloned()
			.unwrap_or_default())
	}
	async fn insert(&mut self, row: &Incident) -> Result<()> {
		self.call("insert");
		self.saved = Some(row.clone());
		Ok(())
	}
	async fn save(&mut self, row: &Incident) -> Result<()> {
		self.call("save");
		let mut saved = row.clone();
		saved.revision += 1;
		self.saved = Some(saved);
		Ok(())
	}
	async fn record_event(&mut self, id: Uuid, actor: &str, change: Value) -> Result<()> {
		assert_eq!(self.saved.as_ref().unwrap().id, id);
		self.call("event");
		if self.repository.pause_event {
			std::future::pending::<()>().await;
		}
		if self.repository.event_failure {
			return Err(Error::External("history unavailable".into()));
		}
		self.events.push((actor.into(), change));
		Ok(())
	}
	async fn events(&mut self, id: Uuid, limit: usize) -> Result<Vec<IncidentEvent>> {
		assert_eq!(id, self.repository.prior.id);
		assert!(matches!(limit, 100 | 500));
		self.call("events");
		Ok([3, 2, 1]
			.into_iter()
			.map(|n| IncidentEvent {
				id: n,
				incident_id: id,
				actor: "source".into(),
				change: json!({"revision":n}),
				created_at: at(n),
			})
			.collect())
	}
	async fn evidence_days(&mut self, tenant: &str) -> Result<i32> {
		assert_eq!(tenant, "tenant");
		self.call("days");
		Ok(2)
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		self.complete()
	}
}
#[async_trait]
impl IncidentRetentionRepository for Repository {
	async fn begin(&self) -> Result<Box<dyn IncidentRetentionScope + '_>> {
		let mut s = self.state.lock().unwrap();
		assert!(!s.active);
		s.active = true;
		Ok(Box::new(Scope {
			repository: self,
			saved: None,
			events: vec![],
			discarded: vec![],
		}))
	}
}
#[async_trait]
impl IncidentRetentionScope for Scope<'_> {
	async fn expired(&mut self) -> Result<Vec<Incident>> {
		self.call("expired");
		Ok(
			if self.repository.state.lock().unwrap().discarded.is_empty() {
				vec![self.repository.prior.clone()]
			} else {
				vec![]
			},
		)
	}
	async fn discard(&mut self, id: Uuid, evidence: Value) -> Result<()> {
		self.call("discard");
		if self.repository.discard_failure {
			return Err(Error::External("retention write unavailable".into()));
		}
		self.discarded.push((id, evidence));
		Ok(())
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		self.complete()
	}
}
#[rstest]
#[tokio::test]
async fn creation_commits_current_manage_authority_record_and_source_history_together() {
	let r = Repository::new();
	let result = create(&r, reference(), new_incident()).await.unwrap();
	assert_eq!(result.revision, 1);
	assert_eq!(result.status, "open");
	assert_eq!(result.severity, "high");
	assert_eq!(
		result.evidence[0]["sha256"],
		"b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
	);
	let s = r.state.lock().unwrap();
	assert_eq!(s.commits, 1);
	assert!(!s.active);
	assert_eq!(s.saved.as_ref().unwrap().id, result.id);
	assert_eq!(
		s.events,
		vec![(
			"reader".into(),
			json!({"created":true,"severity":"high","status":"open","evidence_count":1})
		)]
	);
	assert_eq!(s.evaluations[0].action, "agent_incident.manage");
	assert_eq!(
		s.calls,
		vec![
			"begin",
			"target",
			"inspection",
			"effective",
			"identity",
			"policy",
			"insert",
			"event",
			"read"
		]
	);
}
#[rstest]
#[case("denied")]
#[case("kind")]
#[case("history")]
#[tokio::test]
async fn failed_creation_never_commits_a_record_without_authority_or_history(
	#[case] failure: &str,
) {
	let mut r = Repository::new();
	match failure {
		"denied" => r.allowed = false,
		"kind" => r.kind = "tool",
		_ => r.event_failure = true,
	}
	assert!(create(&r, reference(), new_incident()).await.is_err());
	let s = r.state.lock().unwrap();
	assert_eq!(s.commits, 0);
	assert!(s.saved.is_none() && s.events.is_empty());
	assert!(!s.active);
}
#[rstest]
#[tokio::test]
async fn stale_revision_is_rejected_before_owner_checks_or_writes() {
	let r = Repository::new();
	let mut input = change("resolved");
	input.expected_revision = 4;
	assert!(
		matches!(update(&r,r.prior.id,input).await,Err(Error::Conflict(ref s)) if s=="incident revision changed")
	);
	let s = r.state.lock().unwrap();
	assert_eq!(s.commits, 0);
	assert!(!s.calls.contains(&"target") && !s.calls.contains(&"save"));
}
#[rstest]
#[case("open", "resolved")]
#[case("resolved", "resolved")]
#[case("resolved", "open")]
#[tokio::test]
async fn resolution_preserves_retention_deadlines_and_reopening_clears_them(
	#[case] previous: &str,
	#[case] next: &str,
) {
	let mut r = Repository::new();
	r.prior.status = previous.into();
	if previous == "resolved" {
		r.prior.resolved_at = Some(at(100));
		r.prior.evidence_expires_at = Some(at(200));
	}
	let before = Utc::now();
	let result = update(&r, r.prior.id, change(next)).await.unwrap();
	assert_eq!(result.revision, 6);
	assert_eq!(result.status, next);
	assert_eq!(result.owner, "next");
	let s = r.state.lock().unwrap();
	assert_eq!(s.commits, 1);
	assert_eq!(s.events[0].1["from_revision"], 5);
	if previous == "open" {
		let resolved = result.resolved_at.unwrap();
		assert!(resolved >= before);
		assert_eq!(
			result.evidence_expires_at.unwrap() - resolved,
			chrono::Duration::days(2)
		);
		assert!(s.calls.contains(&"days"));
	} else if next == "resolved" {
		assert_eq!(result.resolved_at, Some(at(100)));
		assert_eq!(result.evidence_expires_at, Some(at(200)));
		assert!(!s.calls.contains(&"days"));
	} else {
		assert_eq!(result.resolved_at, None);
		assert_eq!(result.evidence_expires_at, None);
		assert!(!s.calls.contains(&"days"));
	}
	assert_eq!(s.calls.first().copied(), Some("begin"));
	assert!(s.calls.contains(&"read_locked"));
}
#[rstest]
#[case("expired")]
#[case("capacity")]
#[tokio::test]
async fn copied_evidence_cannot_be_revived_or_exceed_the_total_capacity(#[case] boundary: &str) {
	let mut r = Repository::new();
	let mut input = change("open");
	input.add_evidence = new_incident().evidence;
	if boundary == "expired" {
		r.prior.evidence_expired_at = Some(at(100));
	} else {
		r.prior.evidence = serde_json::to_value(
			(0..32)
				.map(|_| {
					incident::fixed_copy(
						EvidenceInput {
							title: "copy".into(),
							content: "original".into(),
						},
						at(10),
					)
				})
				.collect::<Vec<_>>(),
		)
		.unwrap();
	}
	let error = update(&r, r.prior.id, input).await.unwrap_err();
	if boundary == "expired" {
		assert!(matches!(error, Error::Conflict(_)));
	} else {
		assert!(matches!(error, Error::Invalid(_)));
	}
	let s = r.state.lock().unwrap();
	assert_eq!(s.commits, 0);
	assert!(!s.calls.contains(&"save"));
}
#[rstest]
#[tokio::test]
async fn current_tenant_boundary_rejects_before_identity_lock_or_policy() {
	let mut r = Repository::new();
	r.prior.tenant = "other".into();
	assert!(matches!(get(&r, r.prior.id).await, Err(Error::Forbidden)));
	let s = r.state.lock().unwrap();
	assert!(!s.calls.contains(&"identity") && !s.calls.contains(&"policy"));
	assert_eq!(s.commits, 0);
}
#[rstest]
#[tokio::test]
async fn listing_scans_full_hidden_pages_without_consuming_visible_slots() {
	let mut r = Repository::new();
	let hidden = (1..=100)
		.map(|n| {
			let mut row = incident(n);
			row.tenant = "other".into();
			row
		})
		.collect::<Vec<_>>();
	let last = hidden.last().unwrap();
	let cursor = (last.created_at, last.id);
	r.pages = vec![hidden, vec![incident(1), incident(2), incident(3)]];
	let rows = list(&r, reference()).await.unwrap();
	assert_eq!(
		rows.iter().map(|row| row.id).collect::<Vec<_>>(),
		vec![Uuid::from_u128(1), Uuid::from_u128(3)]
	);
	let s = r.state.lock().unwrap();
	assert_eq!(s.cursors, vec![None, Some(cursor)]);
	assert_eq!(s.commits, 1);
}
#[rstest]
#[tokio::test]
async fn event_history_is_authorized_under_the_locked_incident_and_returned_chronologically() {
	let r = Repository::new();
	let rows = events(&r, r.prior.id).await.unwrap();
	assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 2, 3]);
	let s = r.state.lock().unwrap();
	assert_eq!(
		s.calls,
		vec![
			"begin",
			"read_locked",
			"inspection",
			"identity",
			"policy",
			"events"
		]
	);
	assert_eq!(s.commits, 1);
}
#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn retention_discards_payloads_atomically_while_retaining_source_digest_and_time(
	#[case] fail: bool,
) {
	let mut r = Repository::new();
	r.prior.evidence = serde_json::to_value(vec![incident::fixed_copy(
		EvidenceInput {
			title: "source".into(),
			content: "hello world".into(),
		},
		at(10),
	)])
	.unwrap();
	r.discard_failure = fail;
	let result = purge_expired(&r).await;
	let s = r.state.lock().unwrap();
	if fail {
		assert!(matches!(result, Err(Error::External(_))));
		assert_eq!(s.commits, 0);
		assert!(s.discarded.is_empty());
	} else {
		assert_eq!(result.unwrap(), 1);
		assert_eq!(s.commits, 2);
		assert_eq!(s.discarded[0].0, r.prior.id);
		let copies = s.discarded[0].1.as_array().unwrap();
		assert_eq!(copies[0]["content"], Value::Null);
		assert_eq!(copies[0]["title"], r.prior.evidence[0]["title"]);
		assert_eq!(copies[0]["sha256"], r.prior.evidence[0]["sha256"]);
		assert_eq!(copies[0]["recorded_at"], r.prior.evidence[0]["recorded_at"]);
	}
	assert!(!s.active);
}
#[rstest]
#[tokio::test]
async fn cancelled_history_append_releases_the_uncommitted_record_update() {
	let mut r = Repository::new();
	r.pause_event = true;
	let mut future = Box::pin(update(&r, r.prior.id, change("open")));
	assert!(futures_util::poll!(&mut future).is_pending());
	assert!(r.state.lock().unwrap().active);
	drop(future);
	let s = r.state.lock().unwrap();
	assert!(!s.active);
	assert_eq!(s.commits, 0);
	assert!(s.saved.is_none() && s.events.is_empty());
}
