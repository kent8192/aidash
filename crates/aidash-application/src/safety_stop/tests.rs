use super::*;
use rstest::rstest;
use std::collections::BTreeMap;

struct Fake {
	control: Option<Control>,
	deleted: bool,
	receipts: BTreeMap<(String, Uuid), Stored>,
	applied: Vec<(Action, Actor)>,
	fail_record: bool,
	log: Vec<&'static str>,
}

impl Fake {
	fn with(control: RunControl, revision: i64) -> Self {
		Self {
			control: Some(Control {
				phase: RunPhase::Thinking,
				control,
				control_revision: revision,
			}),
			deleted: false,
			receipts: BTreeMap::new(),
			applied: Vec::new(),
			fail_record: false,
			log: Vec::new(),
		}
	}
}

#[async_trait]
impl Ledger for Fake {
	async fn tenant_available(&mut self) -> Result<()> {
		self.log.push("tenant");
		if self.deleted {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn lock_run(&mut self, _: Uuid) -> Result<Option<Control>> {
		self.log.push("lock");
		Ok(self.control)
	}
	async fn receipt(&mut self, caller: &str, operation: Uuid) -> Result<Option<Stored>> {
		self.log.push("receipt");
		Ok(self.receipts.get(&(caller.to_owned(), operation)).cloned())
	}
	async fn apply(&mut self, _: Uuid, action: Action, actor: Actor) -> Result<()> {
		self.log.push("apply");
		self.applied.push((action, actor));
		if let Some(control) = &mut self.control {
			control.control = action.control();
			control.control_revision += 1;
		}
		Ok(())
	}
	async fn record(&mut self, actor: Actor, command: &Command, outcome: Outcome) -> Result<()> {
		self.log.push("record");
		if self.fail_record {
			return Err(Error::External("audit unavailable".into()));
		}
		self.receipts.insert(
			(actor.caller_key(), command.operation_id),
			Stored {
				fingerprint: command.fingerprint(),
				receipt: Receipt {
					operation_id: command.operation_id,
					run_id: command.run_id,
					tenant: command.tenant.clone(),
					action: command.action,
					outcome,
				},
			},
		);
		Ok(())
	}
}

fn command(action: Action, revision: i64, operation: u128) -> Command {
	Command::new(
		"acme",
		Uuid::from_u128(1),
		action,
		revision,
		Uuid::from_u128(operation),
		"runaway spend",
	)
	.unwrap()
}

#[rstest]
#[case(Action::Pause)]
#[case(Action::Cancel)]
#[tokio::test]
async fn a_current_command_is_applied_and_recorded_once(#[case] action: Action) {
	let mut ledger = Fake::with(RunControl::Active, 2);
	let done = execute(&mut ledger, Actor::SharedBearer, &command(action, 2, 7))
		.await
		.unwrap();
	assert_eq!(done.receipt.outcome, Outcome::Accepted);
	assert_eq!(done.changed, Some(action));
	assert_eq!(ledger.applied, vec![(action, Actor::SharedBearer)]);
	assert_eq!(ledger.log, ["tenant", "lock", "receipt", "apply", "record"]);
}

#[rstest]
#[tokio::test]
async fn an_unknown_run_is_not_found_before_anything_is_recorded() {
	let mut ledger = Fake::with(RunControl::Active, 0);
	ledger.control = None;
	let error = execute(
		&mut ledger,
		Actor::SharedBearer,
		&command(Action::Pause, 0, 7),
	)
	.await
	.unwrap_err();
	assert!(matches!(error, Error::NotFound(_)));
	assert!(ledger.receipts.is_empty() && ledger.applied.is_empty());
}

#[rstest]
#[tokio::test]
async fn a_deleted_tenant_is_refused_before_the_run_is_touched() {
	let mut ledger = Fake::with(RunControl::Active, 0);
	ledger.deleted = true;
	assert!(
		execute(
			&mut ledger,
			Actor::SharedBearer,
			&command(Action::Pause, 0, 7)
		)
		.await
		.is_err()
	);
	assert_eq!(ledger.log, ["tenant"]);
}

#[rstest]
#[tokio::test]
async fn an_identical_retry_replays_the_receipt_without_a_second_effect() {
	let mut ledger = Fake::with(RunControl::Active, 0);
	let first = execute(
		&mut ledger,
		Actor::SharedBearer,
		&command(Action::Pause, 0, 7),
	)
	.await
	.unwrap();
	// The Tenant resumes in between.
	let control = ledger.control.as_mut().unwrap();
	control.control = RunControl::Active;
	control.control_revision += 1;
	let retry = execute(
		&mut ledger,
		Actor::SharedBearer,
		&command(Action::Pause, 0, 7),
	)
	.await
	.unwrap();
	assert_eq!(retry.receipt, first.receipt);
	assert_eq!(retry.changed, None);
	assert_eq!(ledger.applied.len(), 1);
	assert_eq!(ledger.control.unwrap().control, RunControl::Active);
}

#[rstest]
#[tokio::test]
async fn a_changed_command_under_the_same_operation_id_conflicts() {
	let mut ledger = Fake::with(RunControl::Active, 0);
	execute(
		&mut ledger,
		Actor::SharedBearer,
		&command(Action::Pause, 0, 7),
	)
	.await
	.unwrap();
	let error = execute(
		&mut ledger,
		Actor::SharedBearer,
		&command(Action::Cancel, 0, 7),
	)
	.await
	.unwrap_err();
	assert!(matches!(error, Error::Conflict(_)));
	assert_eq!(ledger.applied.len(), 1);
}

#[rstest]
#[tokio::test]
async fn operation_ids_are_scoped_to_the_caller() {
	let mut ledger = Fake::with(RunControl::Active, 0);
	execute(
		&mut ledger,
		Actor::SharedBearer,
		&command(Action::Pause, 0, 7),
	)
	.await
	.unwrap();
	let named = Actor::Browser {
		identity: Uuid::from_u128(5),
	};
	// The same ID from another caller is a new command, and now stale.
	let error = execute(&mut ledger, named, &command(Action::Pause, 0, 7))
		.await
		.unwrap_err();
	assert!(matches!(error, Error::Conflict(_)));
	assert_eq!(ledger.applied.len(), 1);
}

#[rstest]
#[tokio::test]
async fn a_stale_revision_conflicts_without_an_effect() {
	let mut ledger = Fake::with(RunControl::Paused, 4);
	let error = execute(
		&mut ledger,
		Actor::SharedBearer,
		&command(Action::Cancel, 3, 7),
	)
	.await
	.unwrap_err();
	assert!(matches!(error, Error::Conflict(_)));
	assert!(ledger.applied.is_empty() && ledger.receipts.is_empty());
}

#[rstest]
#[tokio::test]
async fn a_pause_of_a_paused_run_is_accepted() {
	let mut ledger = Fake::with(RunControl::Paused, 4);
	let done = execute(
		&mut ledger,
		Actor::SharedBearer,
		&command(Action::Pause, 4, 7),
	)
	.await
	.unwrap();
	assert_eq!(done.receipt.outcome, Outcome::Accepted);
	assert_eq!(ledger.control.unwrap().control_revision, 5);
}

#[rstest]
#[case(RunControl::Cancelled, RunPhase::Thinking)]
#[case(RunControl::Active, RunPhase::Completed)]
#[case(RunControl::Paused, RunPhase::Failed)]
#[tokio::test]
async fn a_cancelled_or_ended_run_is_a_recorded_no_op(
	#[case] control: RunControl,
	#[case] phase: RunPhase,
) {
	let mut ledger = Fake::with(control, 4);
	ledger.control.as_mut().unwrap().phase = phase;
	// Any revision: the no-op needs no fresh read.
	let done = execute(
		&mut ledger,
		Actor::SharedBearer,
		&command(Action::Pause, 0, 7),
	)
	.await
	.unwrap();
	assert_eq!(done.receipt.outcome, Outcome::NoOp);
	assert_eq!(done.changed, None);
	assert!(ledger.applied.is_empty());
	assert_eq!(ledger.receipts.len(), 1);
}

#[rstest]
#[tokio::test]
async fn a_failed_audit_fails_the_command() {
	let mut ledger = Fake::with(RunControl::Active, 0);
	ledger.fail_record = true;
	let error = execute(
		&mut ledger,
		Actor::SharedBearer,
		&command(Action::Pause, 0, 7),
	)
	.await
	.unwrap_err();
	assert!(matches!(error, Error::External(_)));
}
