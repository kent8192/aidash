use super::*;
use aidash_domain::{RunControl, RunMetadata, RunPhase, SnapshotPage, run_input::RunInput};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::{collections::VecDeque, sync::Mutex};
use uuid::Uuid;

struct Scope {
	run: RunMetadata,
	trace: Mutex<Vec<String>>,
	inputs: Mutex<Vec<RunInput>>,
	media: bool,
	accept_error: Mutex<Option<Error>>,
	late_commit: bool,
	promotion_error: Mutex<Option<Error>>,
	reserve_supported: bool,
	promote_supported: bool,
	history: Mutex<VecDeque<Option<Vec<Message>>>>,
	snapshots: Mutex<VecDeque<SnapshotPage>>,
	delivery: Option<Message>,
	acknowledged: Mutex<Vec<Vec<String>>>,
	limits: Mutex<Vec<usize>>,
	bound: Mutex<Vec<(String, Uuid)>>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		run: RunMetadata {
			id: Uuid::new_v4(),
			task_id: Uuid::new_v4(),
			workspace_id: Uuid::new_v4(),
			home_node: "home".into(),
			agent_id: "agent".into(),
			agent_version: "1".into(),
			phase: RunPhase::Thinking,
			control: RunControl::Active,
			step: 0,
			revision: 1,
			observed_input_seq: 0,
			ledger_worker_ready: true,
			error: None,
			lease_owner: None,
			lease_until: None,
			updated_at: chrono::Utc::now(),
		},
		trace: Mutex::new(vec![]),
		inputs: Mutex::new(vec![]),
		media: false,
		accept_error: Mutex::new(None),
		late_commit: false,
		promotion_error: Mutex::new(None),
		reserve_supported: true,
		promote_supported: true,
		history: Mutex::new(VecDeque::from([Some(vec![])])),
		snapshots: Mutex::new(VecDeque::new()),
		delivery: None,
		acknowledged: Mutex::new(vec![]),
		limits: Mutex::new(vec![]),
		bound: Mutex::new(vec![]),
	}
}
impl Scope {
	fn call(&self, label: impl Into<String>) {
		self.trace.lock().unwrap().push(label.into());
	}
	fn admission(&self, sender: &str, content: &str, key: &str) -> Result<()> {
		let error = self.accept_error.lock().unwrap().take();
		if error.is_none() || self.late_commit {
			self.inputs.lock().unwrap().push(RunInput {
				seq: 1,
				sender: sender.into(),
				content: content.into(),
				idempotency_key: key.into(),
				message_id: None,
				reference_only: false,
			});
		}
		if let Some(error) = error {
			Err(error)
		} else {
			Ok(())
		}
	}
}
#[async_trait]
impl RunMessages for Scope {
	fn node(&self) -> &str {
		"executor"
	}
	fn run(&self) -> &RunMetadata {
		&self.run
	}
	async fn inputs(&self) -> Result<Vec<RunInput>> {
		self.call("inputs");
		Ok(self.inputs.lock().unwrap().clone())
	}
	async fn has_media(&self, _: Uuid) -> Result<bool> {
		self.call("media");
		Ok(self.media)
	}
	async fn sequence(&self, key: &str, content: &str) -> Result<i64> {
		self.call("sequence");
		self.inputs
			.lock()
			.unwrap()
			.iter()
			.find(|input| input.idempotency_key == key && input.content == content)
			.map(|input| input.seq)
			.ok_or_else(|| Error::Conflict("no matching ledger entry".into()))
	}
	async fn input_limit(&self) -> Result<usize> {
		self.call("limit");
		Ok(4096)
	}
	async fn accept(&self, sender: &str, content: &str, key: &str, _: usize) -> Result<()> {
		self.call("accept");
		self.admission(sender, content, key)
	}
	async fn import_and_accept(
		&self,
		_: &[(String, Message)],
		sender: &str,
		content: &str,
		key: &str,
		_: usize,
	) -> Result<()> {
		self.call("import-accept");
		self.admission(sender, content, key)
	}
	async fn import_history(&self, history: &[(String, Message)], limit: usize) -> Result<()> {
		self.call("import-history");
		self.limits.lock().unwrap().push(limit);
		for (key, message) in history {
			self.inputs.lock().unwrap().push(RunInput {
				seq: 1,
				sender: message.sender.clone(),
				content: message.content.clone(),
				idempotency_key: key.clone(),
				message_id: Some(message.id),
				reference_only: false,
			});
		}
		Ok(())
	}
	async fn bind(&self, key: &str, message: Uuid) -> Result<()> {
		self.call("bind");
		self.bound.lock().unwrap().push((key.into(), message));
		Ok(())
	}
	async fn reserve(&self, _: &str, _: &str) -> Result<bool> {
		self.call("reserve");
		Ok(self.reserve_supported)
	}
	async fn promote(&self, _: &str, _: &str, sequence: i64) -> Result<bool> {
		self.call(format!("promote:{sequence}"));
		if let Some(error) = self.promotion_error.lock().unwrap().take() {
			Err(error)
		} else {
			Ok(self.promote_supported)
		}
	}
	async fn release(&self, keys: &[String]) -> Result<()> {
		self.call(format!("release:{}", keys.join(",")));
		Ok(())
	}
	async fn acknowledge(&self, keys: &[String]) -> Result<()> {
		self.call(format!("ack:{}", keys.len()));
		self.acknowledged.lock().unwrap().push(keys.to_vec());
		Ok(())
	}
	async fn delivery(&self, _: &str, _: &str) -> Result<Option<Message>> {
		self.call("delivery");
		Ok(self.delivery.clone())
	}
	async fn legacy_message(&self, _: &str, _: &str) -> Result<()> {
		self.call("legacy-message");
		Ok(())
	}
	async fn snapshot_page(&self, _: Option<Uuid>) -> Result<SnapshotPage> {
		self.call("snapshot");
		Ok(self
			.snapshots
			.lock()
			.unwrap()
			.pop_front()
			.unwrap_or(SnapshotPage {
				items: vec![],
				next: None,
			}))
	}
	async fn history_page(&self, offset: usize) -> Result<Option<Vec<Message>>> {
		self.call(format!("history:{offset}"));
		Ok(self
			.history
			.lock()
			.unwrap()
			.pop_front()
			.unwrap_or(Some(vec![])))
	}
	async fn capability(&self) -> Result<Option<Value>> {
		self.call("capability");
		Ok(Some(json!({"protocol":2})))
	}
}
fn input(seq: i64, key: &str) -> RunInput {
	RunInput {
		seq,
		sender: "human".into(),
		content: "correction".into(),
		idempotency_key: key.into(),
		message_id: None,
		reference_only: false,
	}
}
fn message(scope: &Scope, key: &str) -> Message {
	Message {
		id: Uuid::new_v4(),
		workspace_id: scope.run.workspace_id,
		sender: "human".into(),
		content: "correction".into(),
		idempotency_key: Some(remote::full_key(scope.node(), scope.run.task_id, key)),
		created_at: scope.run.updated_at,
	}
}

#[rstest]
#[tokio::test]
async fn reservation_precedes_atomic_admission_and_durable_promotion(scope: Scope) {
	admit(&scope, "human", "correction", "key", 128)
		.await
		.unwrap();
	assert_eq!(
		*scope.trace.lock().unwrap(),
		[
			"inputs",
			"history:0",
			"reserve",
			"import-accept",
			"sequence",
			"promote:1"
		]
	);
	assert_eq!(scope.inputs.lock().unwrap().len(), 1);
}

#[rstest]
#[tokio::test]
async fn late_commit_error_preserves_home_fence_and_binds_its_sequence(mut scope: Scope) {
	// Arrange: the connection failed after the executor committed the correction.
	*scope.accept_error.lock().unwrap() =
		Some(Error::External("connection lost after commit".into()));
	scope.late_commit = true;
	// Act
	admit(&scope, "human", "correction", "key", 128)
		.await
		.unwrap();
	// Assert: both committed-state checks precede promotion; no compensating release.
	assert_eq!(
		*scope.trace.lock().unwrap(),
		[
			"inputs",
			"history:0",
			"reserve",
			"import-accept",
			"sequence",
			"sequence",
			"promote:1"
		]
	);
	assert_eq!(scope.inputs.lock().unwrap().len(), 1);
}

#[rstest]
#[tokio::test]
async fn definitely_uncommitted_input_releases_only_its_original_reservation(scope: Scope) {
	*scope.accept_error.lock().unwrap() = Some(Error::External("admission failed".into()));
	assert!(
		matches!(admit(&scope, "human", "correction", "key", 128).await, Err(Error::External(message)) if message == "admission failed")
	);
	assert_eq!(
		*scope.trace.lock().unwrap(),
		[
			"inputs",
			"history:0",
			"reserve",
			"import-accept",
			"sequence",
			"release:key"
		]
	);
	assert!(scope.inputs.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn recovered_history_rechecks_current_limit_before_binding(scope: Scope) {
	*scope.accept_error.lock().unwrap() = Some(Error::Conflict("ledger already changed".into()));
	*scope.history.lock().unwrap() =
		VecDeque::from([Some(vec![]), Some(vec![message(&scope, "key")])]);
	admit(&scope, "human", "correction", "key", 128)
		.await
		.unwrap();
	assert_eq!(*scope.limits.lock().unwrap(), [4096]);
	assert_eq!(
		*scope.trace.lock().unwrap(),
		[
			"inputs",
			"history:0",
			"reserve",
			"import-accept",
			"limit",
			"history:0",
			"import-history",
			"inputs",
			"sequence",
			"promote:1"
		]
	);
}

#[rstest]
#[tokio::test]
async fn unsupported_reservation_cannot_create_executor_input(mut scope: Scope) {
	scope.reserve_supported = false;
	assert!(
		matches!(admit(&scope, "human", "correction", "key", 128).await, Err(Error::Conflict(message)) if message == "remote home cannot atomically reserve run messages")
	);
	assert_eq!(
		*scope.trace.lock().unwrap(),
		["inputs", "history:0", "reserve"]
	);
	assert!(scope.inputs.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn replay_with_media_is_rejected_before_home_recovery(mut scope: Scope) {
	let mut existing = input(1, "key");
	existing.message_id = Some(Uuid::new_v4());
	scope.inputs.lock().unwrap().push(existing);
	scope.media = true;
	assert!(
		matches!(admit(&scope, "human", "correction", "key", 128).await, Err(Error::Conflict(message)) if message == "run message idempotency key reused with media")
	);
	assert_eq!(*scope.trace.lock().unwrap(), ["inputs", "media"]);
}

#[rstest]
#[tokio::test]
async fn pre_ledger_recovery_promotes_before_attempting_a_new_reservation(scope: Scope) {
	scope.inputs.lock().unwrap().push(input(1, "key"));
	*scope.promotion_error.lock().unwrap() = Some(Error::Conflict("old Home fence expired".into()));
	assert!(recover_home(&scope, "key", "correction").await.unwrap());
	assert_eq!(
		*scope.trace.lock().unwrap(),
		["sequence", "promote:1", "reserve", "sequence", "promote:1"]
	);
}

#[rstest]
#[tokio::test]
async fn delivery_skips_observed_inputs_and_binds_only_valid_records(mut scope: Scope) {
	scope.run.observed_input_seq = 1;
	let mut observed = input(1, "observed");
	observed.message_id = Some(Uuid::new_v4());
	*scope.inputs.lock().unwrap() = vec![observed, input(2, "key")];
	scope.delivery = Some(message(&scope, "key"));
	scope.promote_supported = false;
	deliver(&scope).await.unwrap();
	assert_eq!(
		*scope.bound.lock().unwrap(),
		[("key".into(), scope.delivery.as_ref().unwrap().id)]
	);
	assert_eq!(
		*scope.trace.lock().unwrap(),
		["inputs", "sequence", "promote:2", "delivery", "bind"]
	);
}

#[rstest]
#[tokio::test]
async fn mismatched_delivery_never_binds_an_executor_record(mut scope: Scope) {
	scope.inputs.lock().unwrap().push(input(1, "key"));
	let mut record = message(&scope, "key");
	record.workspace_id = Uuid::new_v4();
	scope.delivery = Some(record);
	assert_eq!(
		deliver(&scope).await.unwrap_err().to_string(),
		"remote run message delivery changed"
	);
	assert!(scope.bound.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn dedicated_history_uses_four_record_pages_and_advancing_offsets(scope: Scope) {
	let first: Vec<_> = (0..4)
		.map(|i| message(&scope, &format!("key-{i}")))
		.collect();
	*scope.history.lock().unwrap() =
		VecDeque::from([Some(first), Some(vec![message(&scope, "last")])]);
	assert_eq!(history(&scope).await.unwrap().len(), 5);
	assert_eq!(*scope.trace.lock().unwrap(), ["history:0", "history:4"]);
}

#[rstest]
#[tokio::test]
async fn full_legacy_snapshot_fails_closed_without_losing_corrections(scope: Scope) {
	*scope.history.lock().unwrap() = VecDeque::from([None]);
	let messages: Vec<_> = (0..100)
		.map(|i| {
			serde_json::to_value(message(&scope, &format!("human:{}:{i}", scope.run.id))).unwrap()
		})
		.collect();
	scope.snapshots.lock().unwrap().push_back(SnapshotPage {
		items: messages,
		next: None,
	});
	assert!(
		matches!(history(&scope).await, Err(Error::External(message)) if message == "legacy peer message snapshot may omit run history")
	);
	assert_eq!(*scope.trace.lock().unwrap(), ["history:0", "snapshot"]);
}

#[rstest]
#[tokio::test]
async fn nonadvancing_legacy_snapshot_is_rejected(scope: Scope) {
	*scope.history.lock().unwrap() = VecDeque::from([None]);
	let page = || SnapshotPage {
		items: vec![],
		next: Some(Uuid::from_u128(2)),
	};
	*scope.snapshots.lock().unwrap() = VecDeque::from([page(), page()]);
	assert!(
		matches!(history(&scope).await, Err(Error::External(message)) if message == "peer snapshot cursor did not advance")
	);
	assert_eq!(
		*scope.trace.lock().unwrap(),
		["history:0", "snapshot", "snapshot"]
	);
}

#[rstest]
#[tokio::test]
async fn only_observed_inputs_are_acknowledged_in_bounded_batches(mut scope: Scope) {
	scope.run.observed_input_seq = 101;
	*scope.inputs.lock().unwrap() = (1..=102).map(|i| input(i, &format!("key-{i}"))).collect();
	acknowledge_observed(&scope).await.unwrap();
	let batches = scope.acknowledged.lock().unwrap();
	assert_eq!(batches.iter().map(Vec::len).collect::<Vec<_>>(), [100, 1]);
	assert_eq!(batches[1], ["key-101"]);
}

#[rstest]
#[tokio::test]
async fn local_delivery_never_contacts_remote_protocol_ports(mut scope: Scope) {
	scope.run.home_node = scope.node().into();
	require_terminal_safe_delivery(&scope).await.unwrap();
	deliver(&scope).await.unwrap();
	assert!(history(&scope).await.unwrap().is_empty());
	acknowledge_observed(&scope).await.unwrap();
	assert!(scope.trace.lock().unwrap().is_empty());
}
