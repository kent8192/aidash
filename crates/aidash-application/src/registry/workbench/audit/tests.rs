use super::*;
use crate::ports::registry::workbench::audit::AuditScope;
use aidash_domain::{
	policy::Decision,
	registry::workbench::{
		Draft,
		audit::{CatalogChange, IncidentChange, Registration, TestRecord},
	},
};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use rstest::rstest;
use std::sync::{Arc, Mutex};
use uuid::Uuid;
struct State {
	active: bool,
	committed: bool,
	calls: Vec<&'static str>,
}
struct Repository {
	principal: Principal,
	state: Arc<Mutex<State>>,
	denied: bool,
	hidden: Vec<Uuid>,
	catalog_allowed: bool,
	incident_denied: bool,
	incident_failure: bool,
	count: usize,
}
impl Repository {
	fn new() -> Self {
		Self {
			principal: Principal::Subject {
				tenant: "tenant".into(),
				subject: "reader".into(),
			},
			state: Arc::new(Mutex::new(State {
				active: false,
				committed: false,
				calls: vec![],
			})),
			denied: false,
			hidden: vec![],
			catalog_allowed: true,
			incident_denied: false,
			incident_failure: false,
			count: 1,
		}
	}
}
struct Scope<'a> {
	repository: &'a Repository,
}
impl Drop for Scope<'_> {
	fn drop(&mut self) {
		self.repository.state.lock().unwrap().active = false;
	}
}
fn at(value: i64) -> DateTime<Utc> {
	Utc.timestamp_opt(value, 0).unwrap()
}
fn reference() -> EntityRef {
	EntityRef {
		id: "agent".into(),
		version: "1".into(),
	}
}
#[async_trait]
impl AuditRepository for Repository {
	fn principal(&self) -> Principal {
		self.principal.clone()
	}
	async fn begin(&self) -> Result<Box<dyn AuditScope + '_>> {
		let mut state = self.state.lock().unwrap();
		state.active = true;
		state.calls.push("begin");
		Ok(Box::new(Scope { repository: self }))
	}
	async fn incidents(&self, entry: &EntityRef) -> Result<Vec<Uuid>> {
		assert_eq!(*entry, reference());
		let mut s = self.state.lock().unwrap();
		assert!(!s.active);
		assert!(s.committed);
		s.calls.push("incidents");
		Ok(vec![Uuid::from_u128(99)])
	}
	async fn incident_events(&self, id: Uuid) -> Result<Vec<IncidentChange>> {
		assert_eq!(id, Uuid::from_u128(99));
		self.state.lock().unwrap().calls.push("incident_events");
		if self.incident_failure {
			return Err(Error::External("incident unavailable".into()));
		}
		if self.incident_denied {
			return Err(Error::Forbidden);
		}
		Ok(vec![IncidentChange {
			actor: "incident owner".into(),
			change: json!({"status":"resolved"}),
			created_at: at(900),
		}])
	}
}
#[async_trait]
impl AuditScope for Scope<'_> {
	async fn require_inspection(&mut self, entry: &EntityRef) -> Result<()> {
		assert_eq!(*entry, reference());
		self.repository
			.state
			.lock()
			.unwrap()
			.calls
			.push("inspection");
		if self.repository.denied {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn registrations(&mut self, _: &EntityRef) -> Result<Vec<Registration>> {
		self.repository
			.state
			.lock()
			.unwrap()
			.calls
			.push("registrations");
		Ok((1..=self.repository.count)
			.map(|i| Registration {
				draft_id: Uuid::from_u128(i as u128),
				revision: 7,
				actor: "registrar".into(),
				registered_at: at(i as i64),
			})
			.collect())
	}
	async fn draft(&mut self, id: Uuid) -> Result<Draft> {
		self.repository.state.lock().unwrap().calls.push("draft");
		Ok(Draft {
			id,
			tenant: if id == Uuid::from_u128(2) {
				"other".into()
			} else {
				"tenant".into()
			},
			owner: "owner".into(),
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
		self.repository
			.state
			.lock()
			.unwrap()
			.calls
			.push("draft_authority");
		if self.repository.hidden.contains(&draft.id) {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn test_records(&mut self, draft: Uuid, revision: i64) -> Result<Vec<TestRecord>> {
		assert_eq!(revision, 7);
		self.repository.state.lock().unwrap().calls.push("test");
		Ok(vec![TestRecord {
			id: draft,
			status: "COMPLETED".into(),
			usage: json!({"tokens":12}),
			created_at: at(500),
		}])
	}
	async fn evaluate(&mut self, tenant: &str, input: &Evaluation) -> Result<Decision> {
		assert_eq!(tenant, "tenant");
		assert_eq!(input.action, "authorization_catalog.history.read");
		assert_eq!(input.resource.id, "agent@1");
		assert_eq!(
			input.resource.attributes,
			json!({"entry_id":"agent","entry_version":"1"})
		);
		self.repository
			.state
			.lock()
			.unwrap()
			.calls
			.push("catalog_authority");
		Ok(Decision {
			allowed: self.repository.catalog_allowed,
			reason: "fixture".into(),
			revision: 1,
			matched_policies: vec![],
			effective_roles: Default::default(),
		})
	}
	async fn catalog_history(&mut self, _: &str, _: &EntityRef) -> Result<Vec<CatalogChange>> {
		self.repository
			.state
			.lock()
			.unwrap()
			.calls
			.push("catalog_history");
		Ok(vec![CatalogChange {
			revision: 3,
			enabled: false,
			actor: "administrator".into(),
			created_at: at(700),
		}])
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		let mut state = self.repository.state.lock().unwrap();
		state.calls.push("commit");
		state.committed = true;
		Ok(())
	}
}
#[rstest]
#[case::offset(10001, None)]
#[case::other_tenant(0, Some("other"))]
#[tokio::test]
async fn invalid_paging_or_cross_tenant_request_is_rejected_before_any_transaction(
	#[case] offset: usize,
	#[case] tenant: Option<&str>,
) {
	let repo = Repository::new();
	assert!(
		read(
			&repo,
			reference(),
			AuditQuery {
				offset,
				tenant: tenant.map(str::to_owned)
			}
		)
		.await
		.is_err()
	);
	assert!(repo.state.lock().unwrap().calls.is_empty());
}
#[rstest]
#[tokio::test]
async fn denied_inspection_cannot_enumerate_drafts_or_any_history() {
	let mut repo = Repository::new();
	repo.denied = true;
	assert!(matches!(
		read(
			&repo,
			reference(),
			AuditQuery {
				offset: 0,
				tenant: None
			}
		)
		.await,
		Err(Error::Forbidden)
	));
	let state = repo.state.lock().unwrap();
	assert_eq!(state.calls, vec!["begin", "inspection"]);
	assert!(!state.active);
	assert!(!state.committed);
}
#[rstest]
#[tokio::test]
async fn factual_sources_are_sorted_and_incident_reads_start_only_after_commit() {
	let repo = Repository::new();
	let page = read(
		&repo,
		reference(),
		AuditQuery {
			offset: 0,
			tenant: None,
		},
	)
	.await
	.unwrap();
	assert_eq!(
		page.items
			.iter()
			.map(|i| i.source.as_str())
			.collect::<Vec<_>>(),
		vec!["incident", "catalog", "sandbox", "registry"]
	);
	assert_eq!(
		page.items[0].details,
		json!({"incident_id":Uuid::from_u128(99),"change":{"status":"resolved"}})
	);
	assert_eq!(
		page.items[1].details,
		json!({"tenant":"tenant","revision":3,"enabled":false})
	);
	assert_eq!(page.items[2].actor, None);
	assert_eq!(page.next_offset, None);
	assert_eq!(
		repo.state.lock().unwrap().calls,
		vec![
			"begin",
			"inspection",
			"registrations",
			"draft",
			"draft_authority",
			"test",
			"catalog_authority",
			"catalog_history",
			"commit",
			"incidents",
			"incident_events"
		]
	);
}
#[rstest]
#[tokio::test]
async fn wrong_tenant_or_hidden_draft_cannot_disclose_its_registration_or_sandbox() {
	let mut repo = Repository::new();
	repo.count = 2;
	repo.hidden = vec![Uuid::from_u128(1)];
	let page = read(
		&repo,
		reference(),
		AuditQuery {
			offset: 0,
			tenant: None,
		},
	)
	.await
	.unwrap();
	assert_eq!(
		page.items
			.iter()
			.map(|i| i.source.as_str())
			.collect::<Vec<_>>(),
		vec!["incident", "catalog"]
	);
	let state = repo.state.lock().unwrap();
	assert!(!state.calls.contains(&"test"));
	assert_eq!(
		state
			.calls
			.iter()
			.filter(|c| **c == "draft_authority")
			.count(),
		1
	);
}
#[rstest]
#[tokio::test]
async fn catalog_and_incident_denials_hide_only_those_sources() {
	let mut repo = Repository::new();
	repo.catalog_allowed = false;
	repo.incident_denied = true;
	let page = read(
		&repo,
		reference(),
		AuditQuery {
			offset: 0,
			tenant: None,
		},
	)
	.await
	.unwrap();
	assert_eq!(
		page.items
			.iter()
			.map(|i| i.source.as_str())
			.collect::<Vec<_>>(),
		vec!["sandbox", "registry"]
	);
	assert!(
		!repo
			.state
			.lock()
			.unwrap()
			.calls
			.contains(&"catalog_history")
	);
}
#[rstest]
#[tokio::test]
async fn incident_storage_failure_is_not_silently_classified_as_hidden_history() {
	let mut repo = Repository::new();
	repo.incident_failure = true;
	assert!(matches!(
		read(
			&repo,
			reference(),
			AuditQuery {
				offset: 0,
				tenant: None
			}
		)
		.await,
		Err(Error::External(_))
	));
	assert!(repo.state.lock().unwrap().committed);
}
#[rstest]
#[case(0, Some(50), 50)]
#[case(50, Some(100), 50)]
#[case(100, None, 20)]
#[tokio::test]
async fn offset_paging_counts_only_visible_sources(
	#[case] offset: usize,
	#[case] next: Option<usize>,
	#[case] length: usize,
) {
	let mut repo = Repository::new();
	repo.count = 60;
	repo.principal = Principal::Operator;
	repo.catalog_allowed = false;
	repo.incident_denied = true;
	let page = read(
		&repo,
		reference(),
		AuditQuery {
			offset,
			tenant: None,
		},
	)
	.await
	.unwrap();
	assert_eq!(page.items.len(), length);
	assert_eq!(page.next_offset, next);
	assert!(
		!repo
			.state
			.lock()
			.unwrap()
			.calls
			.contains(&"catalog_authority")
	);
}
