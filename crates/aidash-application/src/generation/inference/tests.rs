use super::*;
use crate::ports::generation::inference::GenerationInferenceSession;
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::{collections::BTreeMap, sync::Mutex};

#[derive(Clone)]
struct Ledger {
	balances: BTreeMap<Uuid, i64>,
	reports: BTreeMap<Uuid, Option<i64>>,
	allocation: i64,
}
struct State {
	ledger: Ledger,
	requests: Vec<Uuid>,
	calls: Vec<String>,
	fail: Option<String>,
	pause: Option<String>,
	active: usize,
	commits: usize,
}
#[derive(Clone)]
struct World(Arc<Mutex<State>>);
#[fixture]
fn world() -> World {
	let requests = vec![Uuid::from_u128(11), Uuid::from_u128(22)];
	World(Arc::new(Mutex::new(State {
		ledger: Ledger {
			balances: requests.iter().map(|id| (*id, 0)).collect(),
			reports: BTreeMap::new(),
			allocation: 400,
		},
		requests,
		calls: vec![],
		fail: None,
		pause: None,
		active: 0,
		commits: 0,
	})))
}
async fn point(world: &World, name: String) -> Result<()> {
	let (fail, pause) = {
		let mut state = world.0.lock().unwrap();
		state.calls.push(name.clone());
		(
			state.fail.as_ref() == Some(&name),
			state.pause.as_ref() == Some(&name),
		)
	};
	if fail {
		return Err(Error::External("database fault".into()));
	}
	if pause {
		std::future::pending::<()>().await;
	}
	Ok(())
}
struct Authority(World);
struct Origins(Vec<Uuid>);
#[async_trait]
impl GenerationInferenceAuthority for Origins {
	async fn requests(&mut self, node: &str) -> Result<Vec<Uuid>> {
		assert_eq!(node, "aidash://origin");
		Ok(self.0.clone())
	}
}

#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn overlapping_origins_charge_once_and_rollback_as_one_transaction(
	world: World,
	#[case] fail: bool,
) {
	let last = Uuid::from_u128(33);
	{
		let mut state = world.0.lock().unwrap();
		state.ledger.balances.insert(last, 0);
		if fail {
			state.fail = Some(format!("charge:{last}"));
		}
	}
	let mut a = Origins(vec![Uuid::from_u128(11), Uuid::from_u128(22)]);
	let mut b = Origins(vec![Uuid::from_u128(22), last]);
	let result = reserve_many(
		&mut [&mut a, &mut b],
		Arc::new(Repository(world.clone())),
		Uuid::from_u128(77),
		Uuid::from_u128(88),
		100,
		30,
	)
	.await;
	if fail {
		assert!(result.is_err());
		let state = world.0.lock().unwrap();
		assert!(state.ledger.balances.values().all(|value| *value == 0));
		assert!(state.ledger.reports.is_empty());
		assert_eq!(state.commits, 0);
	} else {
		let receipt = result.unwrap().unwrap();
		assert_eq!(
			world
				.0
				.lock()
				.unwrap()
				.ledger
				.balances
				.values()
				.copied()
				.collect::<Vec<_>>(),
			vec![130; 3]
		);
		receipt.settle(&response(70, 20, true)).await.unwrap();
		let state = world.0.lock().unwrap();
		assert_eq!(
			state.ledger.balances.values().copied().collect::<Vec<_>>(),
			vec![90; 3]
		);
		assert_eq!(
			state.ledger.reports.values().copied().collect::<Vec<_>>(),
			vec![Some(90); 3]
		);
		assert_eq!(
			state
				.calls
				.iter()
				.filter(|call| **call == format!("charge:{}", Uuid::from_u128(22)))
				.count(),
			1
		);
	}
}
#[async_trait]
impl GenerationInferenceAuthority for Authority {
	async fn requests(&mut self, node: &str) -> Result<Vec<Uuid>> {
		assert_eq!(node, "aidash://origin");
		point(&self.0, "requests".into()).await?;
		Ok(self.0.0.lock().unwrap().requests.clone())
	}
}
struct Repository(World);
struct Session {
	world: World,
	ledger: Ledger,
	committed: bool,
}
impl Drop for Session {
	fn drop(&mut self) {
		let mut state = self.world.0.lock().unwrap();
		state.active -= 1;
		state
			.calls
			.push(if self.committed { "drop" } else { "rollback" }.into());
	}
}
#[async_trait]
impl GenerationInferenceRepository for Repository {
	fn node_id(&self) -> &str {
		"aidash://origin"
	}
	async fn begin(&self) -> Result<Box<dyn GenerationInferenceSession>> {
		point(&self.0, "begin".into()).await?;
		let mut state = self.0.0.lock().unwrap();
		state.active += 1;
		Ok(Box::new(Session {
			world: self.0.clone(),
			ledger: state.ledger.clone(),
			committed: false,
		}))
	}
}
#[async_trait]
impl GenerationInferenceSession for Session {
	async fn charge(&mut self, request: Uuid, amount: i64) -> Result<()> {
		point(&self.world, format!("charge:{request}")).await?;
		*self.ledger.balances.get_mut(&request).unwrap() += amount;
		Ok(())
	}
	async fn reserve(
		&mut self,
		request: Uuid,
		attempt: Uuid,
		run: Uuid,
		amount: i64,
	) -> Result<()> {
		assert_eq!(attempt, Uuid::from_u128(88));
		assert_eq!(run, Uuid::from_u128(77));
		assert_eq!(amount, 130);
		point(&self.world, format!("reserve:{request}")).await?;
		self.ledger.reports.insert(request, None);
		Ok(())
	}
	async fn refund(&mut self, request: Uuid, amount: i64) -> Result<()> {
		point(&self.world, format!("refund:{request}")).await?;
		*self.ledger.balances.get_mut(&request).unwrap() -= amount;
		Ok(())
	}
	async fn released_policy(&mut self, request: Uuid) -> Result<Option<(String, String)>> {
		point(&self.world, format!("released:{request}")).await?;
		Ok((request == Uuid::from_u128(22)).then(|| ("tenant".into(), "policy".into())))
	}
	async fn refund_allocated(&mut self, tenant: &str, policy: &str, amount: i64) -> Result<()> {
		assert_eq!((tenant, policy), ("tenant", "policy"));
		point(&self.world, "allocation".into()).await?;
		self.ledger.allocation -= amount;
		Ok(())
	}
	async fn report(&mut self, request: Uuid, attempt: Uuid, reported: Option<i64>) -> Result<()> {
		assert_eq!(attempt, Uuid::from_u128(88));
		point(&self.world, format!("report:{request}")).await?;
		*self.ledger.reports.get_mut(&request).unwrap() = reported;
		Ok(())
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		point(&self.world, "commit".into()).await?;
		{
			let mut state = self.world.0.lock().unwrap();
			state.ledger = self.ledger.clone();
			state.commits += 1;
		}
		self.committed = true;
		Ok(())
	}
}
async fn receipt(world: &World) -> Reservation {
	reserve(
		&mut Authority(world.clone()),
		Arc::new(Repository(world.clone())),
		Uuid::from_u128(77),
		Uuid::from_u128(88),
		100,
		30,
	)
	.await
	.unwrap()
	.unwrap()
}
fn response(input: u64, output: u64, complete: bool) -> ModelResponse {
	ModelResponse {
		text: "done".into(),
		tool_calls: vec![],
		input_tokens: input,
		output_tokens: output,
		usage_complete: complete,
	}
}
fn assert_ledger(
	world: &World,
	balance: i64,
	reported: Option<i64>,
	allocation: i64,
	commits: usize,
) {
	let state = world.0.lock().unwrap();
	assert_eq!(state.active, 0);
	assert_eq!(state.commits, commits);
	assert_eq!(state.ledger.allocation, allocation);
	for id in [Uuid::from_u128(11), Uuid::from_u128(22)] {
		assert_eq!(state.ledger.balances[&id], balance);
		assert_eq!(state.ledger.reports[&id], reported);
	}
}
#[rstest]
#[tokio::test]
async fn reservation_commits_both_ancestors_before_returning(world: World) {
	let _reservation = receipt(&world).await;
	assert_ledger(&world, 130, None, 400, 1);
	let expected = vec![
		"requests".into(),
		"begin".into(),
		format!("charge:{}", Uuid::from_u128(11)),
		format!("reserve:{}", Uuid::from_u128(11)),
		format!("charge:{}", Uuid::from_u128(22)),
		format!("reserve:{}", Uuid::from_u128(22)),
		"commit".into(),
		"drop".into(),
	];
	assert_eq!(world.0.lock().unwrap().calls, expected);
}
#[rstest]
#[case((30,10,true,Some(40),40,false))]
#[case((30,10,false,Some(40),130,false))]
#[case((0,0,true,Some(0),130,false))]
#[case((100,30,true,Some(130),130,false))]
#[case((100,31,true,Some(131),130,true))]
#[case((100,31,false,Some(131),130,true))]
#[case((u64::MAX,1,true,None,130,false))]
#[case((i64::MAX as u64+1,0,true,None,130,false))]
#[tokio::test]
async fn settlement_preserves_complete_unknown_and_contract_usage(
	world: World,
	#[case] facts: (u64, u64, bool, Option<i64>, i64, bool),
) {
	let (input, output, complete, reported, balance, exceeded) = facts;
	let reservation = receipt(&world).await;
	let result = reservation.settle(&response(input, output, complete)).await;
	if exceeded {
		assert!(
			matches!(result,Err(Error::External(ref message)) if message=="provider usage exceeded reserved model limits")
		);
	} else {
		result.unwrap();
	}
	assert_ledger(&world, balance, reported, 400 - (130 - balance), 2);
	let calls = &world.0.lock().unwrap().calls;
	if balance == 130 {
		assert!(!calls.iter().any(|name| name.starts_with("released:")));
		assert!(!calls.contains(&"allocation".into()));
	} else {
		assert!(calls.contains(&"allocation".into()));
	}
	assert_eq!(calls.last().unwrap(), "drop");
}
#[rstest]
#[case("requests")]
#[case("begin")]
#[case("charge")]
#[case("reserve")]
#[case("commit")]
#[tokio::test]
async fn failed_reservation_does_not_leave_partial_charge_or_receipt(
	world: World,
	#[case] phase: &str,
) {
	let phase = if ["charge", "reserve"].contains(&phase) {
		format!("{phase}:{}", Uuid::from_u128(22))
	} else {
		phase.into()
	};
	world.0.lock().unwrap().fail = Some(phase);
	let result = reserve(
		&mut Authority(world.clone()),
		Arc::new(Repository(world.clone())),
		Uuid::from_u128(77),
		Uuid::from_u128(88),
		100,
		30,
	)
	.await;
	assert!(matches!(result,Err(Error::External(ref message)) if message=="database fault"));
	let state = world.0.lock().unwrap();
	assert_eq!(state.active, 0);
	assert_eq!(state.commits, 0);
	assert!(state.ledger.reports.is_empty());
	assert!(state.ledger.balances.values().all(|balance| *balance == 0));
}
#[rstest]
#[case("refund")]
#[case("released")]
#[case("allocation")]
#[case("report")]
#[case("commit")]
#[tokio::test]
async fn failed_settlement_keeps_every_committed_reservation(world: World, #[case] phase: &str) {
	let reservation = receipt(&world).await;
	let phase = if ["refund", "released", "report"].contains(&phase) {
		format!("{phase}:{}", Uuid::from_u128(22))
	} else {
		phase.into()
	};
	world.0.lock().unwrap().fail = Some(phase);
	assert!(
		matches!(reservation.settle(&response(30,10,true)).await,Err(Error::External(ref message)) if message=="database fault")
	);
	assert_ledger(&world, 130, None, 400, 1);
	assert_eq!(world.0.lock().unwrap().calls.last().unwrap(), "rollback");
}
#[rstest]
#[case("refund")]
#[case("report")]
#[case("commit")]
#[tokio::test]
async fn canceled_settlement_rolls_back_refunds_without_releasing_durable_usage(
	world: World,
	#[case] phase: &str,
) {
	let reservation = receipt(&world).await;
	let phase = if ["refund", "report"].contains(&phase) {
		format!("{phase}:{}", Uuid::from_u128(22))
	} else {
		phase.into()
	};
	world.0.lock().unwrap().pause = Some(phase);
	let response = response(30, 10, true);
	{
		let future = reservation.settle(&response);
		tokio::pin!(future);
		assert!(futures_util::poll!(future.as_mut()).is_pending());
	}
	assert_ledger(&world, 130, None, 400, 1);
	assert_eq!(world.0.lock().unwrap().calls.last().unwrap(), "rollback");
}
#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn amount_overflow_precedes_begin_unless_there_are_no_generated_ancestors(
	world: World,
	#[case] empty: bool,
) {
	if empty {
		world.0.lock().unwrap().requests.clear();
	}
	let result = reserve(
		&mut Authority(world.clone()),
		Arc::new(Repository(world.clone())),
		Uuid::from_u128(77),
		Uuid::from_u128(88),
		usize::MAX,
		1,
	)
	.await;
	if empty {
		assert!(result.unwrap().is_none());
	} else {
		assert_eq!(
			result.err().unwrap().to_string(),
			"model reservation overflow"
		);
	}
	assert_eq!(world.0.lock().unwrap().calls, vec!["requests"]);
	assert_eq!(world.0.lock().unwrap().commits, 0);
}
