use super::*;
use aidash_domain::transactions::{
	Isolation, Participant, coordination::Vote, management::History,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use serde_json::json;
use std::{
	collections::BTreeMap,
	sync::{
		Arc, Mutex,
		atomic::{AtomicUsize, Ordering},
	},
};

#[fixture]
fn identity() -> ExecutionPrincipal {
	ExecutionPrincipal {
		tenant: "tenant".into(),
		subject: "owner".into(),
		credential_id: Uuid::from_u128(8),
	}
}
fn manifest(id: Uuid) -> Manifest {
	Manifest {
		id,
		coordinator: "aidash://home/airy-hawk-2lu4as22".into(),
		isolation: Isolation::Serializable,
		deadline: "2030-01-01T00:00:00Z".parse().unwrap(),
		participants: vec![Participant {
			node_id: "aidash://peer/airy-hawk-2lu4as22".into(),
			mutations: vec![],
		}],
	}
}
fn status(id: Uuid) -> Status {
	let manifest = manifest(id);
	Status {
		id,
		digest: manifest.digest().unwrap(),
		manifest: json!(manifest),
		decision: None,
		visible: false,
		complete: false,
		last_error: None,
		created_at: "2030-01-01T00:00:00Z".parse().unwrap(),
	}
}
fn failure(category: &str) -> Error {
	match category {
		"forbidden" => Error::Forbidden,
		"unauthorized" => Error::Unauthorized,
		"not_found" => Error::NotFound("transaction".into()),
		"external" => Error::External("peer unavailable".into()),
		"pending" => Error::TransactionPending,
		"identity" => Error::IdentityStatusUnavailable,
		"conflict" => Error::Conflict("state changed".into()),
		"invalid" => Error::Invalid("invalid manifest".into()),
		_ => Error::Port(Box::new(std::io::Error::other("database failure"))),
	}
}
struct State {
	rows: Vec<Status>,
	denials: BTreeMap<Uuid, &'static str>,
	calls: Vec<String>,
	cursors: Vec<Option<(DateTime<Utc>, Uuid)>>,
	failure: Option<&'static str>,
}
#[derive(Clone)]
struct Repository {
	state: Arc<Mutex<State>>,
	active: Arc<AtomicUsize>,
	peak: Arc<AtomicUsize>,
}
#[fixture]
fn repository() -> Repository {
	Repository {
		state: Arc::new(Mutex::new(State {
			rows: vec![status(Uuid::from_u128(1))],
			denials: BTreeMap::new(),
			calls: vec![],
			cursors: vec![],
			failure: None,
		})),
		active: Arc::new(AtomicUsize::new(0)),
		peak: Arc::new(AtomicUsize::new(0)),
	}
}
impl Repository {
	fn event(&self, event: &str) -> Result<()> {
		let mut state = self.state.lock().unwrap();
		state.calls.push(event.into());
		if state.failure == Some(event) {
			return Err(if event == "owner" {
				Error::NotFound("transaction".into())
			} else {
				Error::External(event.into())
			});
		}
		Ok(())
	}
	fn calls(&self) -> Vec<String> {
		self.state.lock().unwrap().calls.clone()
	}
}
struct Active(Arc<AtomicUsize>);
impl Drop for Active {
	fn drop(&mut self) {
		self.0.fetch_sub(1, Ordering::SeqCst);
	}
}
#[async_trait]
impl ManagementRepository for Repository {
	async fn submit_subject(
		&self,
		identity: &ExecutionPrincipal,
		manifest: &Manifest,
	) -> Result<Status> {
		assert_eq!(identity.subject, "owner");
		self.event("submit:subject")?;
		Ok(status(manifest.id))
	}
	async fn candidates(
		&self,
		_identity: &ExecutionPrincipal,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<Status>> {
		self.event("candidates:subject")?;
		let mut state = self.state.lock().unwrap();
		state.cursors.push(cursor);
		Ok(state
			.rows
			.iter()
			.filter(|row| {
				cursor.is_none_or(|(created_at, id)| (row.created_at, row.id) < (created_at, id))
			})
			.take(200)
			.cloned()
			.collect())
	}
	async fn require_owner(&self, identity: &ExecutionPrincipal, _id: Uuid) -> Result<()> {
		assert_eq!(identity.subject, "owner");
		self.event("owner")
	}
	async fn authorize(
		&self,
		identity: &ExecutionPrincipal,
		row: &Status,
		action: &str,
	) -> Result<()> {
		assert_eq!(identity.subject, "owner");
		self.event(action)?;
		let count = self.active.fetch_add(1, Ordering::SeqCst) + 1;
		self.peak.fetch_max(count, Ordering::SeqCst);
		let _active = Active(self.active.clone());
		tokio::task::yield_now().await;
		let mut state = self.state.lock().unwrap();
		if let Some(category) = state.denials.get(&row.id) {
			return Err(failure(category));
		}
		if action == "transaction.abort" {
			state
				.rows
				.iter_mut()
				.find(|stored| stored.id == row.id)
				.unwrap()
				.decision = Some("ABORT".into());
		}
		Ok(())
	}
	async fn status(&self, id: Uuid) -> Result<Status> {
		self.event("status")?;
		self.state
			.lock()
			.unwrap()
			.rows
			.iter()
			.find(|row| row.id == id)
			.cloned()
			.ok_or_else(|| Error::NotFound("transaction".into()))
	}
	async fn history(&self, id: Uuid) -> Result<(Vec<Vote>, Vec<History>)> {
		self.event("history")?;
		Ok((
			vec![Vote {
				node_id: "aidash://peer/airy-hawk-2lu4as22".into(),
				phase: "PREPARED".into(),
			}],
			vec![History {
				sequence: 3,
				transaction_id: id,
				role: "coordinator".into(),
				phase: "COMMIT".into(),
				detail: "".into(),
				created_at: "2030-01-01T00:00:00Z".parse().unwrap(),
			}],
		))
	}
	async fn participants(&self) -> Result<Vec<LocalStatus>> {
		self.event("participants")?;
		Ok(vec![])
	}
}

#[rstest]
#[tokio::test]
async fn authenticated_authority_selects_the_shared_submission_workflow(
	repository: Repository,
	identity: ExecutionPrincipal,
) {
	let manifest = manifest(Uuid::from_u128(1));
	assert_eq!(
		submit(&repository, &identity, &manifest).await.unwrap().id,
		manifest.id
	);
	assert_eq!(repository.calls(), ["submit:subject"]);
}

#[rstest]
#[case("forbidden")]
#[case("unauthorized")]
#[case("not_found")]
#[case("external")]
#[case("pending")]
#[case("identity")]
#[tokio::test]
async fn list_suppresses_only_existing_disclosure_failures(
	repository: Repository,
	identity: ExecutionPrincipal,
	#[case] category: &'static str,
) {
	repository
		.state
		.lock()
		.unwrap()
		.denials
		.insert(Uuid::from_u128(1), category);
	assert!(list(&repository, &identity).await.unwrap().is_empty());
	assert_eq!(
		repository.calls(),
		["candidates:subject", "transaction.read"]
	);
	assert_eq!(repository.active.load(Ordering::SeqCst), 0);
}

#[rstest]
#[case("conflict")]
#[case("invalid")]
#[case("opaque")]
#[tokio::test]
async fn unexpected_contract_and_database_failures_are_not_hidden(
	repository: Repository,
	identity: ExecutionPrincipal,
	#[case] category: &'static str,
) {
	repository
		.state
		.lock()
		.unwrap()
		.denials
		.insert(Uuid::from_u128(1), category);
	let error = list(&repository, &identity).await.unwrap_err();
	assert_eq!(error.to_string(), failure(category).to_string());
	if category == "opaque" {
		assert!(matches!(error, Error::Port(_)));
	}
	assert_eq!(repository.active.load(Ordering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn keyset_paging_fills_200_visible_rows_in_order_with_eight_live_checks(
	repository: Repository,
	identity: ExecutionPrincipal,
) {
	{
		let mut state = repository.state.lock().unwrap();
		state.rows = (1..=400)
			.rev()
			.map(|id| status(Uuid::from_u128(id)))
			.collect();
		state.denials = (301..=400)
			.map(|id| (Uuid::from_u128(id), "forbidden"))
			.collect();
	}
	let rows = list(&repository, &identity).await.unwrap();
	assert_eq!(
		rows.iter().map(|row| row.id).collect::<Vec<_>>(),
		(101..=300).rev().map(Uuid::from_u128).collect::<Vec<_>>()
	);
	assert_eq!(
		repository.state.lock().unwrap().cursors,
		[None, Some((rows[0].created_at, Uuid::from_u128(201)))]
	);
	assert_eq!(repository.peak.load(Ordering::SeqCst), 8);
	assert_eq!(repository.active.load(Ordering::SeqCst), 0);
}

#[rstest]
#[case::owner("owner", & ["owner"])]
#[case::status("status", & ["owner", "status"])]
#[case::disclosure("transaction.read", & ["owner", "status", "transaction.read"])]
#[tokio::test]
async fn details_fail_before_disclosing_vote_or_audit_history(
	repository: Repository,
	identity: ExecutionPrincipal,
	#[case] failure: &'static str,
	#[case] calls: &[&str],
) {
	repository.state.lock().unwrap().failure = Some(failure);
	assert!(
		details(&repository, &identity, Uuid::from_u128(1))
			.await
			.is_err()
	);
	assert_eq!(repository.calls(), calls);
}

#[rstest]
#[tokio::test]
async fn authorized_details_preserve_vote_and_audit_facts(
	repository: Repository,
	identity: ExecutionPrincipal,
) {
	let id = Uuid::from_u128(1);
	let details = details(&repository, &identity, id).await.unwrap();
	assert_eq!(details.transaction, status(id));
	assert_eq!(
		details.participants,
		[Vote {
			node_id: "aidash://peer/airy-hawk-2lu4as22".into(),
			phase: "PREPARED".into()
		}]
	);
	assert_eq!(details.history[0].transaction_id, id);
	assert_eq!(details.history[0].sequence, 3);
	assert_eq!(
		repository.calls(),
		["owner", "status", "transaction.read", "history"]
	);
}

#[rstest]
#[tokio::test]
async fn abort_returns_the_new_decision_after_its_existing_authority_workflow(
	repository: Repository,
	identity: ExecutionPrincipal,
) {
	assert_eq!(
		abort(&repository, &identity, Uuid::from_u128(1))
			.await
			.unwrap()
			.decision
			.as_deref(),
		Some("ABORT")
	);
	assert_eq!(
		repository.calls(),
		["owner", "status", "transaction.abort", "status"]
	);
}

#[rstest]
#[case::participant(Some("aidash://peer/airy-hawk-2lu4as22"), false)]
#[case::outsider(Some("aidash://other/airy-hawk-2lu4as22"), true)]
#[case::missing(None, true)]
#[tokio::test]
async fn immutable_decision_is_disclosed_only_to_a_named_participant(
	repository: Repository,
	#[case] caller: Option<&str>,
	#[case] rejected: bool,
) {
	let result = decision(&repository, caller, Uuid::from_u128(1)).await;
	if rejected {
		assert!(matches!(result, Err(Error::Forbidden)));
	} else {
		assert_eq!(result.unwrap(), status(Uuid::from_u128(1)));
	}
	assert_eq!(repository.calls(), ["status"]);
}

#[rstest]
#[tokio::test]
async fn decision_lookup_and_manifest_errors_precede_caller_filtering(repository: Repository) {
	assert!(matches!(
		decision(&repository, None, Uuid::from_u128(9)).await,
		Err(Error::NotFound(_))
	));
	repository.state.lock().unwrap().rows[0].manifest = json!({});
	assert!(matches!(
		decision(&repository, None, Uuid::from_u128(1)).await,
		Err(Error::Json(_))
	));
}
