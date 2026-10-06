use super::*;
use crate::ports::transactions::management::TrustScope;
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
		coordinator: "aidash://home".into(),
		isolation: Isolation::Serializable,
		deadline: "2030-01-01T00:00:00Z".parse().unwrap(),
		participants: vec![Participant {
			node_id: "aidash://peer".into(),
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
	trusts: Vec<Trust>,
	pending: Vec<Uuid>,
	scopes: usize,
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
			trusts: vec![Trust {
				node_id: "aidash://peer".into(),
				enabled: true,
			}],
			pending: vec![Uuid::from_u128(1)],
			scopes: 0,
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
struct TrustMutation(Repository);
impl Drop for TrustMutation {
	fn drop(&mut self) {
		let mut state = self.0.state.lock().unwrap();
		state.scopes -= 1;
		state.calls.push("release:trust".into());
	}
}
#[async_trait]
impl TrustScope for TrustMutation {
	async fn set(&mut self, trust: Trust) -> Result<Trust> {
		self.0.event("set:trust")?;
		self.0.state.lock().unwrap().trusts = vec![trust.clone()];
		Ok(trust)
	}
	async fn pending_peer(&mut self, node: &str) -> Result<Vec<Uuid>> {
		assert_eq!(self.0.state.lock().unwrap().scopes, 1);
		self.0.pending_peer(node).await
	}
}
#[async_trait]
impl ManagementRepository for Repository {
	async fn submit_operator(&self, manifest: &Manifest) -> Result<Status> {
		self.event("submit:operator")?;
		Ok(status(manifest.id))
	}
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
		identity: Option<&ExecutionPrincipal>,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<Status>> {
		self.event(if identity.is_some() {
			"candidates:subject"
		} else {
			"candidates:operator"
		})?;
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
				node_id: "aidash://peer".into(),
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
	async fn abort_operator(&self, id: Uuid) -> Result<Status> {
		self.event("abort:operator")?;
		let mut row = status(id);
		row.decision = Some("ABORT".into());
		Ok(row)
	}
	async fn participants(&self) -> Result<Vec<LocalStatus>> {
		self.event("participants")?;
		Ok(vec![])
	}
	async fn trusts(&self) -> Result<Vec<Trust>> {
		self.event("trusts")?;
		Ok(self.state.lock().unwrap().trusts.clone())
	}
	async fn ensure_peer(&self, node: &str) -> Result<()> {
		assert_eq!(node, "aidash://peer");
		self.event("peer")
	}
	async fn trust_scope(&self) -> Result<Box<dyn TrustScope>> {
		self.event("open:trust")?;
		self.state.lock().unwrap().scopes += 1;
		Ok(Box::new(TrustMutation(self.clone())))
	}
	async fn pending_peer(&self, node: &str) -> Result<Vec<Uuid>> {
		assert_eq!(node, "aidash://peer");
		self.event("pending")?;
		Ok(self.state.lock().unwrap().pending.clone())
	}
	fn credential(&self, reference: &str) -> Result<String> {
		assert_eq!(reference, "PEER_FIXTURE_KEY");
		self.event("credential")?;
		Ok("fixture-key".into())
	}
	async fn restore_peer(&self, node: &str, reference: &str, credential: &str) -> Result<Peer> {
		self.event("restore")?;
		assert_eq!(credential, "fixture-key");
		Ok(Peer {
			node_id: node.into(),
			endpoint: "http://127.0.0.1".into(),
			credential_env: reference.into(),
			protocol_version: "0.1".into(),
			enabled: true,
		})
	}
}

#[rstest]
#[case::operator(false, "submit:operator")]
#[case::subject(true, "submit:subject")]
#[tokio::test]
async fn authenticated_authority_selects_the_shared_submission_workflow(
	repository: Repository,
	identity: ExecutionPrincipal,
	#[case] subject: bool,
	#[case] call: &str,
) {
	let manifest = manifest(Uuid::from_u128(1));
	assert_eq!(
		submit(&repository, subject.then_some(&identity), &manifest)
			.await
			.unwrap()
			.id,
		manifest.id
	);
	assert_eq!(repository.calls(), [call]);
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
	assert!(list(&repository, Some(&identity)).await.unwrap().is_empty());
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
	let error = list(&repository, Some(&identity)).await.unwrap_err();
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
	let rows = list(&repository, Some(&identity)).await.unwrap();
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
#[tokio::test]
async fn operator_list_uses_durable_rows_without_subject_disclosure(repository: Repository) {
	repository
		.state
		.lock()
		.unwrap()
		.denials
		.insert(Uuid::from_u128(1), "forbidden");
	assert_eq!(list(&repository, None).await.unwrap().len(), 1);
	assert_eq!(repository.calls(), ["candidates:operator"]);
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
		details(&repository, Some(&identity), Uuid::from_u128(1))
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
	let details = details(&repository, Some(&identity), id).await.unwrap();
	assert_eq!(details.transaction, status(id));
	assert_eq!(
		details.participants,
		[Vote {
			node_id: "aidash://peer".into(),
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
#[case::subject(true, & ["owner", "status", "transaction.abort", "status"])]
#[case::operator(false, & ["abort:operator"])]
#[tokio::test]
async fn abort_returns_the_new_decision_after_its_existing_authority_workflow(
	repository: Repository,
	identity: ExecutionPrincipal,
	#[case] subject: bool,
	#[case] calls: &[&str],
) {
	assert_eq!(
		abort(
			&repository,
			subject.then_some(&identity),
			Uuid::from_u128(1)
		)
		.await
		.unwrap()
		.decision
		.as_deref(),
		Some("ABORT")
	);
	assert_eq!(repository.calls(), calls);
}

#[rstest]
#[case::enabled(true, & ["peer", "open:trust", "set:trust", "release:trust"], vec![])]
#[case::disabled(false, & ["open:trust", "set:trust", "pending", "release:trust"], vec![Uuid::from_u128(1)])]
#[tokio::test]
async fn disabling_trust_keeps_the_pending_scan_after_the_durable_change(
	repository: Repository,
	#[case] enabled: bool,
	#[case] calls: &[&str],
	#[case] pending: Vec<Uuid>,
) {
	let result = trust(
		&repository,
		Trust {
			node_id: "aidash://peer".into(),
			enabled,
		},
	)
	.await
	.unwrap();
	assert_eq!(result.pending_transactions, pending);
	assert_eq!(result.trust.enabled, enabled);
	assert_eq!(repository.state.lock().unwrap().scopes, 0);
	assert_eq!(repository.calls(), calls);
}

#[rstest]
#[case::set("set:trust", true)]
#[case::pending("pending", false)]
#[tokio::test]
async fn trust_scope_releases_on_error_without_undoing_a_committed_change(
	repository: Repository,
	#[case] failure: &'static str,
	#[case] persisted_enabled: bool,
) {
	repository.state.lock().unwrap().failure = Some(failure);
	assert!(
		trust(
			&repository,
			Trust {
				node_id: "aidash://peer".into(),
				enabled: false
			}
		)
		.await
		.is_err()
	);
	assert_eq!(
		repository.state.lock().unwrap().trusts[0].enabled,
		persisted_enabled
	);
	assert_eq!(repository.state.lock().unwrap().scopes, 0);
	assert_eq!(repository.calls().last().unwrap(), "release:trust");
}

#[rstest]
#[tokio::test]
async fn enabling_trust_requires_a_current_enabled_peer_before_opening_storage(
	repository: Repository,
) {
	repository.state.lock().unwrap().failure = Some("peer");
	assert!(
		trust(
			&repository,
			Trust {
				node_id: "aidash://peer".into(),
				enabled: true
			}
		)
		.await
		.is_err()
	);
	assert_eq!(repository.calls(), ["peer"]);
}

#[rstest]
#[tokio::test]
async fn trust_listing_checks_pending_obligations_only_for_disabled_peers(repository: Repository) {
	repository.state.lock().unwrap().trusts.push(Trust {
		node_id: "aidash://peer".into(),
		enabled: false,
	});
	let result = trust_list(&repository).await.unwrap();
	assert!(result[0].pending_transactions.is_empty());
	assert_eq!(result[1].pending_transactions, [Uuid::from_u128(1)]);
	assert_eq!(repository.calls(), ["trusts", "pending"]);
}

#[rstest]
#[case::valid(false, & ["credential", "restore"])]
#[case::invalid(true, & ["credential"])]
#[tokio::test]
async fn peer_recovery_resolves_the_key_before_native_restoration(
	repository: Repository,
	#[case] rejected: bool,
	#[case] calls: &[&str],
) {
	if rejected {
		repository.state.lock().unwrap().failure = Some("credential");
	}
	let result = restore_peer(&repository, "aidash://peer", "PEER_FIXTURE_KEY").await;
	assert_eq!(result.is_err(), rejected);
	assert_eq!(repository.calls(), calls);
}

#[rstest]
#[case::participant(Some("aidash://peer"), false)]
#[case::outsider(Some("aidash://other"), true)]
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
