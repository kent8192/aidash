use super::*;
use crate::{
	generation::test_support,
	ports::generation::settlement::{GenerationSettlementScope, GenerationSettlementSession},
};
use aidash_domain::{
	generation::{
		remote::{Attempt, Purpose, ReservedCharge},
		requests::Request,
	},
	registry::EntityRef,
	semantic::remote::Provider,
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::sync::{Arc, Mutex};

#[fixture]
fn usage() -> Usage {
	Usage {
		operation_id: Uuid::from_u128(1),
		attempt_id: Uuid::from_u128(2),
		dispatcher_node: "aidash://edge".into(),
		grant_id: Uuid::from_u128(3),
		admission_id: Uuid::from_u128(4),
		purpose: Purpose::Inference,
		provider: Provider {
			node_id: "aidash://edge".into(),
			entry: EntityRef {
				id: "model".into(),
				version: "1.0.0".into(),
			},
			digest: format!("sha256:{}", "a".repeat(64)),
			configuration_digest: format!("sha256:{}", "b".repeat(64)),
		},
		input_digest: format!("sha256:{}", "c".repeat(64)),
		reserved_tokens: 100,
	}
}
struct State {
	attempt: Attempt,
	rows: Vec<ReservedCharge>,
	jobs: Vec<Request>,
	calls: Vec<String>,
	committed: Vec<String>,
	budget_rows: u64,
	policy_rows: u64,
	fail: Option<&'static str>,
	pause: Option<&'static str>,
	active: usize,
}
#[derive(Clone)]
struct Repository(Arc<Mutex<State>>);
#[fixture]
fn repository(usage: Usage) -> Repository {
	let mut first = test_support::request(1);
	first.id = Uuid::from_u128(11);
	let mut second = first.clone();
	second.id = Uuid::from_u128(22);
	second.quota_released = true;
	Repository(Arc::new(Mutex::new(State {
		attempt: Attempt {
			digest: usage.digest().unwrap(),
			result: None,
		},
		rows: [first.id, second.id]
			.into_iter()
			.map(|request_id| ReservedCharge {
				request_id,
				state: "RESERVED".into(),
				reported_tokens: None,
			})
			.collect(),
		jobs: vec![first, second],
		calls: vec![],
		committed: vec![],
		budget_rows: 1,
		policy_rows: 1,
		fail: None,
		pause: None,
		active: 0,
	})))
}
impl Repository {
	fn calls(&self) -> Vec<String> {
		self.0.lock().unwrap().calls.clone()
	}
}
async fn point(state: &Arc<Mutex<State>>, name: &str) -> Result<()> {
	let (fail, pause) = {
		let mut state = state.lock().unwrap();
		state.calls.push(name.into());
		(state.fail == Some(name), state.pause == Some(name))
	};
	if fail {
		return Err(Error::Port(Box::new(std::io::Error::other(
			"database unavailable",
		))));
	}
	if pause {
		std::future::pending::<()>().await;
	}
	Ok(())
}
struct Session {
	state: Arc<Mutex<State>>,
	effects: Vec<String>,
	rows: Vec<ReservedCharge>,
	result: Option<Value>,
	committed: bool,
}
impl Drop for Session {
	fn drop(&mut self) {
		let mut state = self.state.lock().unwrap();
		state.active -= 1;
		state
			.calls
			.push(if self.committed { "drop" } else { "rollback" }.into());
	}
}
#[async_trait]
impl GenerationSettlementRepository for Repository {
	async fn begin(&self) -> Result<Box<dyn GenerationSettlementSession>> {
		point(&self.0, "begin").await?;
		let mut state = self.0.lock().unwrap();
		state.active += 1;
		Ok(Box::new(Session {
			state: self.0.clone(),
			effects: vec![],
			rows: state.rows.clone(),
			result: None,
			committed: false,
		}))
	}
}
#[async_trait]
impl GenerationSettlementSession for Session {
	async fn commit(mut self: Box<Self>) -> Result<()> {
		point(&self.state, "commit").await?;
		{
			let mut state = self.state.lock().unwrap();
			state.committed.append(&mut self.effects);
			state.rows = self.rows.clone();
			state.attempt.result = self.result.take();
		}
		self.committed = true;
		Ok(())
	}
}
#[async_trait]
impl GenerationSettlementScope for Session {
	async fn lock_attempt(&mut self, attempt: Uuid, _: &str) -> Result<Attempt> {
		assert_eq!(attempt, Uuid::from_u128(2));
		point(&self.state, "lock").await?;
		Ok(self.state.lock().unwrap().attempt.clone())
	}
	async fn reservations(&mut self, _: Uuid, digest: &str) -> Result<Vec<ReservedCharge>> {
		point(&self.state, "rows").await?;
		assert_eq!(digest, self.state.lock().unwrap().attempt.digest);
		Ok(self.rows.clone())
	}
	async fn refund_budget(
		&mut self,
		id: Uuid,
		tokens: i64,
		purpose: Purpose,
		release_call: bool,
	) -> Result<u64> {
		point(&self.state, "budget").await?;
		self.effects.push(format!(
			"budget:{id}:{tokens}:{}:{release_call}",
			purpose.name()
		));
		Ok(self.state.lock().unwrap().budget_rows)
	}
	async fn request(&mut self, id: Uuid) -> Result<Request> {
		point(&self.state, "request").await?;
		Ok(self
			.state
			.lock()
			.unwrap()
			.jobs
			.iter()
			.find(|job| job.id == id)
			.unwrap()
			.clone())
	}
	async fn refund_policy(
		&mut self,
		job: &Request,
		tokens: i64,
		purpose: Purpose,
		release_call: bool,
	) -> Result<u64> {
		point(&self.state, "policy").await?;
		assert!(job.quota_released);
		self.effects.push(format!(
			"policy:{}:{tokens}:{}:{release_call}",
			job.id,
			purpose.name()
		));
		Ok(self.state.lock().unwrap().policy_rows)
	}
	async fn settle_reservation(
		&mut self,
		id: Uuid,
		_: Uuid,
		state: &str,
		reported: Option<i64>,
	) -> Result<()> {
		point(&self.state, "settle").await?;
		let row = self
			.rows
			.iter_mut()
			.find(|row| row.request_id == id)
			.unwrap();
		row.state = state.into();
		row.reported_tokens = reported;
		self.effects
			.push(format!("settle:{id}:{state}:{reported:?}"));
		Ok(())
	}
	async fn save_finalization(&mut self, _: Uuid, result: &Value) -> Result<()> {
		point(&self.state, "save").await?;
		self.result = Some(result.clone());
		self.effects.push("save".into());
		Ok(())
	}
}

#[rstest]
#[case(Finalization::Aborted {}, 100, true, None, false)]
#[case(Finalization::Settled { reported: None }, 0, false, None, false)]
#[case(Finalization::Settled { reported: Some(0) }, 0, false, Some(0), false)]
#[case(Finalization::Settled { reported: Some(-1) }, 0, false, Some(-1), false)]
#[case(Finalization::Settled { reported: Some(40) }, 60, false, Some(40), false)]
#[case(Finalization::Settled { reported: Some(100) }, 0, false, Some(100), false)]
#[case(Finalization::Settled { reported: Some(101) }, 0, false, Some(101), true)]
#[tokio::test]
async fn bounded_accounting_is_atomic_for_active_and_released_quota(
	repository: Repository,
	usage: Usage,
	#[case] result: Finalization,
	#[case] refund: i64,
	#[case] release_call: bool,
	#[case] reported: Option<i64>,
	#[case] violated: bool,
) {
	let actual = finalize(&repository, &usage, &result).await;
	if violated {
		assert!(matches!(
			actual,
			Err(Error::RemoteSemantic(Failure::ProviderContract))
		));
	} else {
		actual.unwrap();
	}
	let state = repository.0.lock().unwrap();
	assert_eq!(state.attempt.result, Some(json!(result)));
	let expected_state = if release_call { "RELEASED" } else { "SETTLED" };
	assert_eq!(
		state.committed,
		[
			format!(
				"budget:{}:{refund}:inference:{release_call}",
				Uuid::from_u128(11)
			),
			format!(
				"settle:{}:{expected_state}:{reported:?}",
				Uuid::from_u128(11)
			),
			format!(
				"budget:{}:{refund}:inference:{release_call}",
				Uuid::from_u128(22)
			),
			format!(
				"policy:{}:{refund}:inference:{release_call}",
				Uuid::from_u128(22)
			),
			format!(
				"settle:{}:{expected_state}:{reported:?}",
				Uuid::from_u128(22)
			),
			"save".into(),
		]
	);
	assert_eq!(
		state.calls,
		[
			"begin", "lock", "rows", "budget", "request", "settle", "budget", "request", "policy",
			"settle", "save", "commit", "drop"
		]
	);
	assert_eq!(state.active, 0);
}

#[rstest]
#[case(Purpose::Embedding)]
#[case(Purpose::Compaction)]
#[case(Purpose::Inference)]
#[tokio::test]
async fn abort_preserves_purpose_for_each_atomic_call_refund(
	repository: Repository,
	mut usage: Usage,
	#[case] purpose: Purpose,
) {
	usage.purpose = purpose;
	repository.0.lock().unwrap().attempt.digest = usage.digest().unwrap();
	finalize(&repository, &usage, &Finalization::Aborted {})
		.await
		.unwrap();
	let state = repository.0.lock().unwrap();
	for effect in state
		.committed
		.iter()
		.filter(|effect| effect.starts_with("budget:") || effect.starts_with("policy:"))
	{
		assert!(effect.ends_with(&format!(":100:{}:true", purpose.name())));
	}
	assert_eq!(state.attempt.result, Some(json!({"state":"aborted"})));
}

#[rstest]
#[case(0)]
#[case(2)]
#[tokio::test]
async fn budget_refund_requires_exactly_one_row(
	repository: Repository,
	usage: Usage,
	#[case] rows: u64,
) {
	repository.0.lock().unwrap().budget_rows = rows;
	let error = finalize(&repository, &usage, &Finalization::Aborted {})
		.await
		.unwrap_err();
	assert_eq!(
		error.to_string(),
		"provider refund exceeds committed reservation"
	);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls, ["begin", "lock", "rows", "budget", "rollback"]);
	assert!(state.committed.is_empty());
	assert!(state.attempt.result.is_none());
}

#[rstest]
#[case(0)]
#[case(2)]
#[tokio::test]
async fn policy_refund_failure_rolls_back_preceding_request_refunds(
	repository: Repository,
	usage: Usage,
	#[case] rows: u64,
) {
	repository.0.lock().unwrap().policy_rows = rows;
	let error = finalize(&repository, &usage, &Finalization::Aborted {})
		.await
		.unwrap_err();
	assert_eq!(
		error.to_string(),
		"provider refund exceeds policy allocation"
	);
	let state = repository.0.lock().unwrap();
	assert!(
		state.calls.contains(&"settle".into()),
		"first ancestor reached a provisional settlement"
	);
	assert_eq!(state.calls.last().unwrap(), "rollback");
	assert!(state.committed.is_empty());
	assert!(state.rows.iter().all(|row| row.state == "RESERVED"));
}

#[rstest]
#[tokio::test]
async fn changed_attempt_digest_stops_before_reservation_reads(
	repository: Repository,
	usage: Usage,
) {
	repository.0.lock().unwrap().attempt.digest = "different".into();
	let error = finalize(&repository, &usage, &Finalization::Aborted {})
		.await
		.unwrap_err();
	assert_eq!(
		error.to_string(),
		"provider attempt has a different reservation"
	);
	assert_eq!(repository.calls(), ["begin", "lock", "rollback"]);
}

#[rstest]
#[tokio::test]
async fn different_finalization_stops_before_any_refund(repository: Repository, usage: Usage) {
	repository.0.lock().unwrap().attempt.result = Some(json!({"state":"settled","reported":40}));
	let error = finalize(&repository, &usage, &Finalization::Aborted {})
		.await
		.unwrap_err();
	assert_eq!(
		error.to_string(),
		"provider attempt already finalized differently"
	);
	assert_eq!(repository.calls(), ["begin", "lock", "rollback"]);
}

#[rstest]
#[case("SETTLED", Some(40))]
#[case("RELEASED", Some(1))]
#[case("UNKNOWN", None)]
#[tokio::test]
async fn conflicting_reservation_replay_cannot_refund(
	repository: Repository,
	usage: Usage,
	#[case] old: &str,
	#[case] reported: Option<i64>,
) {
	{
		let mut state = repository.0.lock().unwrap();
		state.rows[0].state = old.into();
		state.rows[0].reported_tokens = reported;
	}
	let error = finalize(&repository, &usage, &Finalization::Aborted {})
		.await
		.unwrap_err();
	assert_eq!(
		error.to_string(),
		"provider attempt already finalized differently"
	);
	assert_eq!(repository.calls(), ["begin", "lock", "rows", "rollback"]);
}

#[rstest]
#[case(Finalization::Aborted {})]
#[case(Finalization::Settled { reported: Some(40) })]
#[case(Finalization::Settled { reported: Some(101) })]
#[tokio::test]
async fn exact_replay_never_refunds_twice(
	repository: Repository,
	usage: Usage,
	#[case] result: Finalization,
) {
	let decision = result.accounting(usage.reserved_tokens);
	{
		let mut state = repository.0.lock().unwrap();
		state.attempt.result = Some(json!(result));
		for row in &mut state.rows {
			row.state = decision.state.into();
			row.reported_tokens = decision.reported;
		}
	}
	let actual = finalize(&repository, &usage, &result).await;
	if decision.provider_contract_violated {
		assert!(matches!(
			actual,
			Err(Error::RemoteSemantic(Failure::ProviderContract))
		));
	} else {
		actual.unwrap();
	}
	let state = repository.0.lock().unwrap();
	assert_eq!(
		state.calls,
		["begin", "lock", "rows", "save", "commit", "drop"]
	);
	assert_eq!(state.committed, ["save"]);
	assert_eq!(state.attempt.result, Some(json!(result)));
}

#[rstest]
#[tokio::test]
async fn finalization_before_reservation_still_saves_attempt_decision(
	repository: Repository,
	usage: Usage,
) {
	repository.0.lock().unwrap().rows.clear();
	finalize(&repository, &usage, &Finalization::Aborted {})
		.await
		.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(state.committed, ["save"]);
	assert_eq!(state.attempt.result, Some(json!({"state":"aborted"})));
}

#[rstest]
#[case("lock")]
#[case("rows")]
#[case("budget")]
#[case("request")]
#[case("policy")]
#[case("settle")]
#[case("save")]
#[case("commit")]
#[tokio::test]
async fn adapter_failure_rolls_back_the_entire_settlement(
	repository: Repository,
	usage: Usage,
	#[case] at: &'static str,
) {
	repository.0.lock().unwrap().fail = Some(at);
	assert!(matches!(
		finalize(&repository, &usage, &Finalization::Aborted {}).await,
		Err(Error::Port(_))
	));
	let state = repository.0.lock().unwrap();
	assert_eq!(state.active, 0);
	assert!(state.committed.is_empty());
	assert!(state.attempt.result.is_none());
	assert!(state.rows.iter().all(|row| row.state == "RESERVED"));
	assert_eq!(state.calls.last().unwrap(), "rollback");
}

#[rstest]
#[case("budget")]
#[case("policy")]
#[case("save")]
#[case("commit")]
#[tokio::test]
async fn cancellation_rolls_back_refunds_and_releases_owned_transaction(
	repository: Repository,
	usage: Usage,
	#[case] at: &'static str,
) {
	repository.0.lock().unwrap().pause = Some(at);
	assert!(
		tokio::time::timeout(
			std::time::Duration::from_millis(10),
			finalize(&repository, &usage, &Finalization::Aborted {})
		)
		.await
		.is_err()
	);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.active, 0);
	assert!(state.committed.is_empty());
	assert!(state.rows.iter().all(|row| row.state == "RESERVED"));
	assert!(state.attempt.result.is_none());
}

#[rstest]
#[tokio::test]
async fn invalid_usage_is_rejected_before_starting_transaction(
	repository: Repository,
	mut usage: Usage,
) {
	usage.attempt_id = Uuid::nil();
	assert!(matches!(
		finalize(&repository, &usage, &Finalization::Aborted {}).await,
		Err(Error::RemoteSemantic(Failure::ProviderContract))
	));
	assert_eq!(repository.calls(), Vec::<String>::new());
}
