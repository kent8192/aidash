use super::*;
use crate::ports::generation::dispatch::{DispatchPreparation, DispatchVisibility};
use aidash_domain::{
	generation::remote::{Purpose, Usage},
	registry::EntityRef,
	semantic::remote::Provider,
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::Value;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
enum Fault {
	Forbidden,
	ProviderContract,
	External,
}
impl Fault {
	fn error(self) -> Error {
		match self {
			Self::Forbidden => Error::Forbidden,
			Self::ProviderContract => Error::RemoteSemantic(Failure::ProviderContract),
			Self::External => Error::Port(Box::new(std::io::Error::other("adapter unavailable"))),
		}
	}
}
struct State {
	record: Option<Record>,
	calls: Vec<String>,
	admit_rows: u64,
	finalize_rows: u64,
	writes: Vec<(String, String, Value)>,
	pending: Vec<Uuid>,
	fail: Option<(&'static str, Fault)>,
	pause: Option<&'static str>,
	visibility: usize,
	preparations: usize,
	peer_ack: bool,
}
#[derive(Clone)]
struct Repository(Arc<Mutex<State>>);

#[fixture]
fn input() -> Input {
	let node = "aidash://dispatch".to_owned();
	Input {
		usage: Usage {
			operation_id: Uuid::from_u128(1),
			attempt_id: Uuid::from_u128(2),
			dispatcher_node: node.clone(),
			grant_id: Uuid::from_u128(3),
			admission_id: Uuid::from_u128(4),
			purpose: Purpose::Inference,
			provider: Provider {
				node_id: node,
				entry: EntityRef {
					id: "model".into(),
					version: "1.0.0".into(),
				},
				digest: format!("sha256:{}", "a".repeat(64)),
				configuration_digest: format!("sha256:{}", "b".repeat(64)),
			},
			input_digest: format!("sha256:{}", "c".repeat(64)),
			reserved_tokens: 4096,
		},
		boundary: json!({"step": 7}),
	}
}
fn record(input: &Input) -> Record {
	Record {
		usage: json!(input.usage),
		digest: input.usage.digest().unwrap(),
		peer_node: "aidash://home".into(),
		boundary: input.boundary.clone(),
		state: "PREPARING".into(),
		finalization: None,
		peer_finalized: false,
	}
}
#[fixture]
fn repository() -> Repository {
	Repository(Arc::new(Mutex::new(State {
		record: None,
		calls: vec![],
		admit_rows: 1,
		finalize_rows: 1,
		writes: vec![],
		pending: vec![],
		fail: None,
		pause: None,
		visibility: 0,
		preparations: 0,
		peer_ack: true,
	})))
}
impl Repository {
	fn terminal(&self, input: &Input, result: Finalization) {
		let mut record = record(input);
		record.state = match result {
			Finalization::Aborted {} => "ABORTED",
			Finalization::Settled { .. } => "SETTLED",
		}
		.into();
		record.finalization = Some(json!(result));
		self.0.lock().unwrap().record = Some(record);
	}
	fn calls(&self) -> Vec<String> {
		self.0.lock().unwrap().calls.clone()
	}
}
async fn point(state: &Arc<Mutex<State>>, name: &str) -> Result<()> {
	let (failure, pause) = {
		let mut state = state.lock().unwrap();
		state.calls.push(name.into());
		(
			state
				.fail
				.filter(|(at, _)| *at == name)
				.map(|(_, fault)| fault),
			state.pause == Some(name),
		)
	};
	if let Some(fault) = failure {
		return Err(fault.error());
	}
	if pause {
		std::future::pending::<()>().await;
	}
	Ok(())
}
struct Preparation {
	state: Arc<Mutex<State>>,
	staged: Option<Record>,
	committed: bool,
}
impl Drop for Preparation {
	fn drop(&mut self) {
		let mut state = self.state.lock().unwrap();
		state.preparations -= 1;
		state.calls.push(
			if self.committed {
				"preparation_drop"
			} else {
				"rollback"
			}
			.into(),
		);
	}
}
#[async_trait]
impl DispatchPreparation for Preparation {
	async fn insert(&mut self, input: &Input, peer: &str, digest: &str) -> Result<()> {
		point(&self.state, "insert").await?;
		let mut existing = self
			.state
			.lock()
			.unwrap()
			.record
			.clone()
			.unwrap_or_else(|| record(input));
		if self.state.lock().unwrap().record.is_none() {
			existing.peer_node = peer.into();
			existing.digest = digest.into();
		}
		self.staged = Some(existing);
		Ok(())
	}
	async fn record(&mut self, _: Uuid) -> Result<Record> {
		point(&self.state, "locked_record").await?;
		Ok(self.staged.clone().unwrap())
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		point(&self.state, "commit").await?;
		self.state.lock().unwrap().record = self.staged.take();
		self.committed = true;
		Ok(())
	}
}
struct Visibility {
	state: Arc<Mutex<State>>,
	held: bool,
}
impl Drop for Visibility {
	fn drop(&mut self) {
		let mut state = self.state.lock().unwrap();
		if self.held {
			state.visibility -= 1;
		}
		state.calls.push("visibility_drop".into());
	}
}
#[async_trait]
impl DispatchVisibility for Visibility {
	async fn terminal_record(&mut self, _: Uuid) -> Result<Option<Record>> {
		point(&self.state, "terminal_record").await?;
		Ok(self
			.state
			.lock()
			.unwrap()
			.record
			.clone()
			.filter(|r| matches!(r.state.as_str(), "ABORTED" | "SETTLED")))
	}
	async fn abort_stale_preparations(&mut self) -> Result<()> {
		point(&self.state, "abort_stale").await
	}
	async fn pending(&mut self) -> Result<Vec<Uuid>> {
		point(&self.state, "pending").await?;
		Ok(self.state.lock().unwrap().pending.clone())
	}
	async fn suspend(&mut self) -> Result<()> {
		point(&self.state, "suspend").await?;
		self.state.lock().unwrap().visibility -= 1;
		self.held = false;
		Ok(())
	}
}
#[async_trait]
impl GenerationDispatchRepository for Repository {
	fn node_id(&self) -> &str {
		"aidash://dispatch"
	}
	async fn begin_preparation(&self) -> Result<Box<dyn DispatchPreparation>> {
		point(&self.0, "begin_preparation").await?;
		self.0.lock().unwrap().preparations += 1;
		Ok(Box::new(Preparation {
			state: self.0.clone(),
			staged: None,
			committed: false,
		}))
	}
	async fn record(&self, _: Uuid) -> Result<Option<Record>> {
		point(&self.0, "record").await?;
		Ok(self.0.lock().unwrap().record.clone())
	}
	async fn admit(&self, input: &Input, receipts: &[Reserved]) -> Result<u64> {
		point(&self.0, "admit").await?;
		assert!(receipts.is_empty());
		assert_eq!(input.usage.attempt_id, Uuid::from_u128(2));
		Ok(self.0.lock().unwrap().admit_rows)
	}
	async fn finalize(&self, _: Uuid, from: &str, state: &str, value: &Value) -> Result<u64> {
		point(&self.0, "persist").await?;
		let mut data = self.0.lock().unwrap();
		data.writes.push((from.into(), state.into(), value.clone()));
		if data.finalize_rows == 1 {
			let record = data.record.as_mut().unwrap();
			record.state = state.into();
			record.finalization = Some(value.clone());
		}
		Ok(data.finalize_rows)
	}
	async fn begin_visibility(&self) -> Result<Box<dyn DispatchVisibility>> {
		point(&self.0, "begin_visibility").await?;
		self.0.lock().unwrap().visibility += 1;
		Ok(Box::new(Visibility {
			state: self.0.clone(),
			held: true,
		}))
	}
	async fn mark_peer_finalized(&self, _: Uuid) -> Result<()> {
		point(&self.0, "acknowledge").await?;
		self.0
			.lock()
			.unwrap()
			.record
			.as_mut()
			.unwrap()
			.peer_finalized = true;
		Ok(())
	}
}
#[async_trait]
impl GenerationDispatchSettlement for Repository {
	async fn local(&self, usage: &Usage, result: &Finalization) -> Result<()> {
		{
			let data = self.0.lock().unwrap();
			assert_eq!(
				data.visibility, 1,
				"local refund remains visibility-protected"
			);
			assert_eq!(data.record.as_ref().unwrap().usage, json!(usage));
			assert_eq!(
				data.record.as_ref().unwrap().finalization,
				Some(json!(result)),
				"decision is durable before refund"
			);
		}
		point(&self.0, "local").await
	}
	async fn peer(&self, node: &str, input: &FinalizeInput) -> Result<bool> {
		{
			let data = self.0.lock().unwrap();
			assert_eq!(
				data.visibility, 0,
				"peer I/O cannot retain local visibility"
			);
			let record = data.record.as_ref().unwrap();
			assert_eq!(record.peer_node, node);
			assert_eq!(record.usage, json!(input.usage));
			assert_eq!(record.finalization, Some(json!(input.result)));
		}
		point(&self.0, "peer").await?;
		Ok(self.0.lock().unwrap().peer_ack)
	}
}

#[rstest]
#[tokio::test]
async fn preparation_commits_only_after_exact_locked_binding(repository: Repository, input: Input) {
	prepare(&repository, &input, "aidash://home").await.unwrap();
	assert_eq!(
		repository.calls(),
		[
			"begin_preparation",
			"insert",
			"locked_record",
			"commit",
			"preparation_drop"
		]
	);
	assert_eq!(
		repository.0.lock().unwrap().record.as_ref().unwrap().usage,
		json!(input.usage)
	);
}

#[rstest]
#[case("digest")]
#[case("boundary")]
#[case("peer")]
#[case("usage")]
#[tokio::test]
async fn preparation_rejects_changed_binding_without_commit(
	repository: Repository,
	input: Input,
	#[case] field: &str,
) {
	let mut previous = record(&input);
	match field {
		"digest" => previous.digest = "changed".into(),
		"boundary" => previous.boundary = json!({"step":8}),
		"peer" => previous.peer_node = "aidash://other".into(),
		"usage" => previous.usage["reserved_tokens"] = json!(1),
		_ => unreachable!(),
	}
	repository.0.lock().unwrap().record = Some(previous);
	assert!(matches!(
		prepare(&repository, &input, "aidash://home").await,
		Err(Error::Conflict(_))
	));
	assert_eq!(
		repository.calls(),
		["begin_preparation", "insert", "locked_record", "rollback"]
	);
}

#[rstest]
#[case("DISPATCHED")]
#[case("ABORTED")]
#[case("SETTLED")]
#[tokio::test]
async fn preparation_cannot_cross_a_prior_dispatch(
	repository: Repository,
	input: Input,
	#[case] state: &str,
) {
	let mut previous = record(&input);
	previous.state = state.into();
	repository.0.lock().unwrap().record = Some(previous);
	assert!(matches!(
		prepare(&repository, &input, "aidash://home").await,
		Err(Error::RemoteSemantic(Failure::Pending))
	));
	assert_eq!(repository.calls().last().unwrap(), "rollback");
}

#[rstest]
#[case(65536, true)]
#[case(65537, false)]
#[tokio::test]
async fn admission_boundary_retains_encoded_byte_limit(
	repository: Repository,
	mut input: Input,
	#[case] bytes: usize,
	#[case] allowed: bool,
) {
	input.boundary = json!("x".repeat(bytes - 2));
	assert_eq!(serde_json::to_vec(&input.boundary).unwrap().len(), bytes);
	let result = prepare(&repository, &input, "aidash://home").await;
	assert_eq!(result.is_ok(), allowed);
	if !allowed {
		assert!(matches!(result, Err(Error::Forbidden)));
		assert_eq!(repository.calls(), Vec::<String>::new());
	}
}

#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn admission_rejects_identity_or_contract_before_transaction(
	repository: Repository,
	mut input: Input,
	#[case] invalid_contract: bool,
) {
	if invalid_contract {
		input.usage.attempt_id = Uuid::nil();
	} else {
		input.usage.dispatcher_node = "aidash://other".into();
		input.usage.provider.node_id = input.usage.dispatcher_node.clone();
	}
	let result = prepare(&repository, &input, "aidash://home").await;
	if invalid_contract {
		assert!(matches!(
			result,
			Err(Error::RemoteSemantic(Failure::ProviderContract))
		));
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
	}
	assert_eq!(repository.calls(), Vec::<String>::new());
}

#[rstest]
#[case("insert")]
#[case("locked_record")]
#[case("commit")]
#[tokio::test]
async fn cancelled_preparation_releases_owned_transaction(
	repository: Repository,
	input: Input,
	#[case] at: &'static str,
) {
	repository.0.lock().unwrap().pause = Some(at);
	assert!(
		tokio::time::timeout(
			std::time::Duration::from_millis(10),
			prepare(&repository, &input, "aidash://home")
		)
		.await
		.is_err()
	);
	let data = repository.0.lock().unwrap();
	assert_eq!(data.preparations, 0);
	assert!(data.record.is_none());
	assert_eq!(data.calls.last().unwrap(), "rollback");
}

#[rstest]
#[case("absent")]
#[case("digest")]
#[case("boundary")]
#[case("usage")]
#[tokio::test]
async fn bound_denies_missing_or_mismatched_admission(
	repository: Repository,
	input: Input,
	#[case] field: &str,
) {
	let mut previous = record(&input);
	match field {
		"digest" => previous.digest = "changed".into(),
		"boundary" => previous.boundary = Value::Null,
		"usage" => previous.usage = Value::Null,
		"absent" => {}
		_ => unreachable!(),
	}
	if field != "absent" {
		repository.0.lock().unwrap().record = Some(previous);
	}
	assert!(matches!(
		bound(&repository, &input).await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.calls(), ["record"]);
}

#[rstest]
#[tokio::test]
async fn bound_preserves_peer_and_state_for_caller_authorization(
	repository: Repository,
	input: Input,
) {
	let mut previous = record(&input);
	previous.peer_node = "aidash://different".into();
	previous.state = "ABORTED".into();
	repository.0.lock().unwrap().record = Some(previous);
	let actual = bound(&repository, &input).await.unwrap();
	assert_eq!(actual.peer_node, "aidash://different");
	assert_eq!(actual.state, "ABORTED");
}

#[rstest]
#[case(0, false)]
#[case(1, true)]
#[case(2, false)]
#[tokio::test]
async fn admission_requires_exactly_one_atomic_update(
	repository: Repository,
	input: Input,
	#[case] rows: u64,
	#[case] allowed: bool,
) {
	repository.0.lock().unwrap().admit_rows = rows;
	let result = admitted(&repository, &input, &[]).await;
	assert_eq!(result.is_ok(), allowed);
	if !allowed {
		assert!(matches!(
			result,
			Err(Error::RemoteSemantic(Failure::Pending))
		));
	}
	assert_eq!(repository.calls(), ["admit"]);
}

#[rstest]
#[case(Finalization::Aborted {}, "PREPARING", "ABORTED")]
#[case(Finalization::Settled { reported: Some(123) }, "DISPATCHED", "SETTLED")]
#[tokio::test]
async fn finalization_persists_exact_transition_before_either_refund(
	repository: Repository,
	input: Input,
	#[case] result: Finalization,
	#[case] from: &str,
	#[case] to: &str,
) {
	let mut previous = record(&input);
	previous.state = from.into();
	repository.0.lock().unwrap().record = Some(previous);
	finish(&repository, &repository, &input, result.clone())
		.await
		.unwrap();
	let data = repository.0.lock().unwrap();
	assert_eq!(data.writes, [(from.into(), to.into(), json!(result))]);
	assert_eq!(
		data.calls,
		[
			"record",
			"persist",
			"begin_visibility",
			"terminal_record",
			"local",
			"suspend",
			"peer",
			"acknowledge",
			"visibility_drop"
		]
	);
	assert!(data.record.as_ref().unwrap().peer_finalized);
}

#[rstest]
#[case(Finalization::Aborted {})]
#[case(Finalization::Settled { reported: None })]
#[tokio::test]
async fn exact_terminal_replay_can_redeliver_after_failed_cas(
	repository: Repository,
	input: Input,
	#[case] result: Finalization,
) {
	repository.terminal(&input, result.clone());
	repository.0.lock().unwrap().finalize_rows = 0;
	finish(&repository, &repository, &input, result)
		.await
		.unwrap();
	assert!(
		repository
			.0
			.lock()
			.unwrap()
			.record
			.as_ref()
			.unwrap()
			.peer_finalized
	);
}

#[rstest]
#[case("PREPARING", None)]
#[case("ABORTED", Some(json!({"state":"settled","reported":3})))]
#[case("SETTLED", Some(json!({"state":"aborted"})))]
#[tokio::test]
async fn different_terminal_decision_cannot_issue_refunds(
	repository: Repository,
	input: Input,
	#[case] state: &str,
	#[case] value: Option<Value>,
) {
	let mut previous = record(&input);
	previous.state = state.into();
	previous.finalization = value;
	{
		let mut data = repository.0.lock().unwrap();
		data.record = Some(previous);
		data.finalize_rows = 0;
	}
	assert!(matches!(
		finish(&repository, &repository, &input, Finalization::Aborted {}).await,
		Err(Error::Conflict(_))
	));
	assert_eq!(repository.calls(), ["record", "persist"]);
}

#[rstest]
#[case("usage")]
#[case("finalization")]
#[case("missing_finalization")]
#[tokio::test]
async fn corrupt_terminal_payload_releases_lease_without_refund(
	repository: Repository,
	input: Input,
	#[case] field: &str,
) {
	repository.terminal(&input, Finalization::Aborted {});
	{
		let mut data = repository.0.lock().unwrap();
		let record = data.record.as_mut().unwrap();
		match field {
			"usage" => record.usage = Value::Null,
			"finalization" => record.finalization = Some(Value::Null),
			"missing_finalization" => record.finalization = None,
			_ => unreachable!(),
		}
	}
	let result = deliver(&repository, &repository, input.usage.attempt_id).await;
	if field == "missing_finalization" {
		assert!(matches!(result, Err(Error::Forbidden)));
	} else {
		assert!(matches!(result, Err(Error::Json(_))));
	}
	assert_eq!(
		repository.calls(),
		["begin_visibility", "terminal_record", "visibility_drop"]
	);
	assert_eq!(repository.0.lock().unwrap().visibility, 0);
}

#[rstest]
#[tokio::test]
async fn local_failure_still_delivers_peer_decision_but_never_acknowledges(
	repository: Repository,
	input: Input,
) {
	repository.terminal(&input, Finalization::Aborted {});
	repository.0.lock().unwrap().fail = Some(("local", Fault::Forbidden));
	assert!(matches!(
		deliver(&repository, &repository, input.usage.attempt_id).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		repository.calls(),
		[
			"begin_visibility",
			"terminal_record",
			"local",
			"suspend",
			"peer",
			"visibility_drop"
		]
	);
	assert!(
		!repository
			.0
			.lock()
			.unwrap()
			.record
			.as_ref()
			.unwrap()
			.peer_finalized
	);
}

#[rstest]
#[tokio::test]
async fn provider_contract_failure_is_durable_and_acknowledged_but_still_returned(
	repository: Repository,
	input: Input,
) {
	repository.terminal(
		&input,
		Finalization::Settled {
			reported: Some(9000),
		},
	);
	repository.0.lock().unwrap().fail = Some(("local", Fault::ProviderContract));
	assert!(matches!(
		deliver(&repository, &repository, input.usage.attempt_id).await,
		Err(Error::RemoteSemantic(Failure::ProviderContract))
	));
	assert!(
		repository
			.0
			.lock()
			.unwrap()
			.record
			.as_ref()
			.unwrap()
			.peer_finalized
	);
	assert!(repository.calls().contains(&"acknowledge".into()));
}

#[rstest]
#[case("suspend")]
#[case("peer")]
#[case("acknowledge")]
#[tokio::test]
async fn delivery_failure_retains_pending_outbox(
	repository: Repository,
	input: Input,
	#[case] at: &'static str,
) {
	repository.terminal(&input, Finalization::Aborted {});
	repository.0.lock().unwrap().fail = Some((at, Fault::External));
	assert!(matches!(
		deliver(&repository, &repository, input.usage.attempt_id).await,
		Err(Error::Port(_))
	));
	let data = repository.0.lock().unwrap();
	assert_eq!(data.visibility, 0);
	assert!(!data.record.as_ref().unwrap().peer_finalized);
	assert_eq!(
		data.record.as_ref().unwrap().finalization,
		Some(json!({"state":"aborted"}))
	);
}

#[rstest]
#[case("local")]
#[case("suspend")]
#[case("peer")]
#[tokio::test]
async fn cancelled_delivery_releases_visibility_without_acknowledging(
	repository: Repository,
	input: Input,
	#[case] at: &'static str,
) {
	repository.terminal(&input, Finalization::Aborted {});
	repository.0.lock().unwrap().pause = Some(at);
	assert!(
		tokio::time::timeout(
			std::time::Duration::from_millis(10),
			deliver(&repository, &repository, input.usage.attempt_id)
		)
		.await
		.is_err()
	);
	let data = repository.0.lock().unwrap();
	assert_eq!(data.visibility, 0);
	assert!(!data.record.as_ref().unwrap().peer_finalized);
}

#[rstest]
#[tokio::test]
async fn false_peer_wire_reply_retains_existing_acknowledgment_contract(
	repository: Repository,
	input: Input,
) {
	repository.terminal(&input, Finalization::Aborted {});
	repository.0.lock().unwrap().peer_ack = false;
	deliver(&repository, &repository, input.usage.attempt_id)
		.await
		.unwrap();
	assert!(
		repository
			.0
			.lock()
			.unwrap()
			.record
			.as_ref()
			.unwrap()
			.peer_finalized
	);
}

#[rstest]
#[tokio::test]
async fn acknowledged_peer_is_skipped_but_local_settlement_is_retried(
	repository: Repository,
	input: Input,
) {
	repository.terminal(&input, Finalization::Aborted {});
	repository
		.0
		.lock()
		.unwrap()
		.record
		.as_mut()
		.unwrap()
		.peer_finalized = true;
	deliver(&repository, &repository, input.usage.attempt_id)
		.await
		.unwrap();
	assert_eq!(
		repository.calls(),
		[
			"begin_visibility",
			"terminal_record",
			"local",
			"suspend",
			"visibility_drop"
		]
	);
}

#[rstest]
#[tokio::test]
async fn recovery_suspends_scan_lease_and_continues_after_failed_delivery(
	repository: Repository,
	input: Input,
) {
	repository.terminal(&input, Finalization::Aborted {});
	{
		let mut data = repository.0.lock().unwrap();
		data.pending = vec![input.usage.attempt_id, Uuid::from_u128(5)];
		data.fail = Some(("peer", Fault::External));
	}
	reconcile(&repository, &repository).await.unwrap();
	let data = repository.0.lock().unwrap();
	assert_eq!(
		&data.calls[..4],
		["begin_visibility", "abort_stale", "pending", "suspend"]
	);
	assert_eq!(data.calls.iter().filter(|name| *name == "peer").count(), 2);
	assert_eq!(data.visibility, 0);
	assert!(!data.record.as_ref().unwrap().peer_finalized);
}

#[rstest]
#[case("abort_stale")]
#[case("pending")]
#[case("suspend")]
#[tokio::test]
async fn recovery_scan_failure_propagates_and_releases_visibility(
	repository: Repository,
	#[case] at: &'static str,
) {
	repository.0.lock().unwrap().fail = Some((at, Fault::External));
	assert!(matches!(
		reconcile(&repository, &repository).await,
		Err(Error::Port(_))
	));
	assert_eq!(repository.0.lock().unwrap().visibility, 0);
	assert!(!repository.calls().contains(&"local".into()));
}
