use super::*;
fn file() -> FileEntry {
	FileEntry {
		file_id: Uuid::new_v4(),
		path: "report.txt".into(),
		digest: "digest".into(),
		size: 5,
		media_type: "text/plain".into(),
		scope: FileScope::Working,
		provenance: json!({}),
	}
}
fn limits() -> Limits {
	Limits {
		files: 4,
		file_bytes: 8,
		bytes: 12,
		admission: true,
	}
}
fn select(file: &FileEntry) -> Selection {
	Selection {
		file_id: file.file_id,
		expected_digest: file.digest.clone(),
	}
}
#[rstest::rstest]
fn selected_order_and_immutable_metadata_are_preserved() {
	let a = file();
	let mut b = file();
	b.path = "other.txt".into();
	let (selected, bytes) = select_files(
		&[a.clone(), b.clone()],
		&[select(&b), select(&a)],
		&limits(),
	)
	.unwrap();
	assert_eq!(
		selected.iter().map(|f| f.file_id).collect::<Vec<_>>(),
		vec![b.file_id, a.file_id]
	);
	assert_eq!(bytes, 10);
	assert_eq!(selected[0].digest, b.digest);
}
#[rstest::rstest]
fn missing_selection_remains_concealed() {
	let f = file();
	assert!(matches!(
		select_files(&[], &[select(&f)], &limits()),
		Err(Error::NotFound(_))
	));
}
#[rstest::rstest]
fn changed_digest_cannot_be_snapshotted() {
	let f = file();
	let mut selection = select(&f);
	selection.expected_digest = "old".into();
	assert!(
		matches!(select_files(&[f],&[selection],&limits()),Err(Error::Conflict(code)) if code=="FILE_CHANGED: report.txt")
	);
}
#[rstest::rstest]
fn duplicate_selection_is_rejected() {
	let f = file();
	assert!(
		matches!(select_files(std::slice::from_ref(&f),&[select(&f),select(&f)],&limits()),Err(Error::Invalid(code)) if code=="SHARE_FILE_LIMIT")
	);
}
#[rstest::rstest]
fn a_file_cannot_exceed_its_budget() {
	let mut f = file();
	f.size = 9;
	assert!(
		matches!(select_files(&[f.clone()],&[select(&f)],&limits()),Err(Error::Invalid(code)) if code=="SHARE_FILE_LIMIT")
	);
}
#[rstest::rstest]
fn the_complete_snapshot_must_fit_the_total_budget() {
	let a = file();
	let b = file();
	let mut l = limits();
	l.bytes = 9;
	assert!(
		matches!(select_files(&[a.clone(),b.clone()],&[select(&a),select(&b)],&l),Err(Error::Invalid(code)) if code=="SHARE_BYTE_LIMIT")
	);
}
#[rstest::rstest]
fn a_reference_file_cannot_be_shared_to_another_agent() {
	let mut reference = file();
	reference.scope = FileScope::References;
	reference.provenance = json!({"kind":"reference","id":Uuid::new_v4()});
	assert!(matches!(
		select_files(
			std::slice::from_ref(&reference),
			&[select(&reference)],
			&limits()
		),
		Err(Error::NotFound(_))
	));
}
#[rstest::rstest]
fn a_copy_of_a_reference_file_as_made_cannot_be_shared() {
	let mut reference = file();
	reference.scope = FileScope::References;
	reference.provenance = json!({"kind":"reference","id":Uuid::new_v4()});
	let mut copy = reference.clone();
	copy.file_id = Uuid::new_v4();
	copy.scope = FileScope::Working;
	copy.path = "copy.txt".into();
	let mut again = file();
	again.path = "again.txt".into();
	again.provenance = json!({"kind":"operation","operation_id":"cp"});
	let mut edited = again.clone();
	edited.file_id = Uuid::new_v4();
	edited.path = "edited.txt".into();
	edited.digest = "edited".into();
	let mounted = [reference, copy.clone(), again.clone(), edited.clone()];

	for refused in [&copy, &again] {
		assert!(
			matches!(
				select_files(&mounted, &[select(refused)], &limits()),
				Err(Error::NotFound(_))
			),
			"{} left the Run",
			refused.path
		);
	}
	let (selected, _) = select_files(&mounted, &[select(&edited)], &limits()).unwrap();
	assert_eq!(selected[0].file_id, edited.file_id);
}

// A Run executing for another Home sends files back to that Home only (ADR 0139).
use aidash_domain::capabilities::{sessions::remote_home_constraint, sharing::Recipient};
use async_trait::async_trait;

const HOME: &str = "aidash://home/node";
const HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

struct Receiving {
	limits: (usize, u64, u64),
	replay: Option<Value>,
	active: Option<Uuid>,
	permitted: bool,
	reads: usize,
	contents: Vec<u8>,
}
fn receiving(run: &RunMetadata) -> Receiving {
	Receiving {
		limits: (4, 100, 200),
		replay: None,
		active: Some(run.id),
		permitted: true,
		reads: 0,
		contents: b"hello".to_vec(),
	}
}
#[async_trait]
impl HomeShareScope for Receiving {
	fn share_limits(&self) -> Result<Limits> {
		Ok(Limits {
			files: self.limits.0,
			file_bytes: self.limits.1,
			bytes: self.limits.2,
			admission: true,
		})
	}
	async fn replay(&mut self, _: Uuid, _: &str) -> Result<Option<Value>> {
		Ok(self.replay.clone())
	}
	async fn active_run(&mut self, _: &Area) -> Result<Option<Uuid>> {
		Ok(self.active)
	}
	async fn permit(&mut self, _: &Area, action: &str) -> Result<()> {
		assert_eq!(action, "file.share");
		if self.permitted {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
	async fn read_range(&mut self, _: &FileEntry, offset: u64) -> Result<Vec<u8>> {
		self.reads += 1;
		Ok(self.contents[offset as usize..].to_vec())
	}
}
fn home_run() -> RunMetadata {
	RunMetadata {
		node: "brave-otter-k3m9q2xa".parse().unwrap(),
		id: Uuid::new_v4(),
		task_id: Uuid::new_v4(),
		thread_id: Uuid::new_v4(),
		home_node: HOME.into(),
		agent_id: "agent".into(),
		agent_version: "1".into(),
		phase: aidash_domain::RunPhase::Ready,
		control: aidash_domain::RunControl::Active,
		step: 0,
		revision: 1,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		control_revision: 0,
		control_source: Default::default(),
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: chrono::Utc::now(),
	}
}
fn receiving_area(files: &[FileEntry], extra: &[Value]) -> Area {
	let mut constraints = vec![remote_home_constraint(HOME)];
	constraints.extend(extra.iter().cloned());
	Area {
		id: Uuid::new_v4(),
		tenant: "tenant".into(),
		home_node: HOME.into(),
		thread_id: Uuid::new_v4(),
		agent_id: "agent".into(),
		owner: "person".into(),
		generation: 1,
		revision: 4,
		epoch: 1,
		state: "active".into(),
		manifest: json!(files),
		constraints: json!(constraints),
		next_sequence: 1,
	}
}
fn hello() -> FileEntry {
	FileEntry {
		file_id: Uuid::new_v4(),
		path: "hello.txt".into(),
		digest: HELLO.into(),
		size: 5,
		media_type: "text/plain".into(),
		scope: FileScope::Working,
		provenance: json!({"kind":"operation"}),
	}
}
fn share_to(node: &str, files: &[&FileEntry]) -> Share {
	Share {
		idempotency_key: Uuid::new_v4(),
		expected_revision: 4,
		files: files.iter().map(|f| select(f)).collect(),
		recipient: Recipient {
			node_id: node.into(),
			agent_id: "agent".into(),
			agent_version: "1".into(),
			thread_id: Uuid::new_v4(),
		},
	}
}

#[tokio::test]
async fn the_snapshot_a_remote_run_sends_home_is_read_whole_and_verified() {
	let run = home_run();
	let file = hello();
	let area = receiving_area(std::slice::from_ref(&file), &[]);
	let mut scope = receiving(&run);
	let HomeShare::Deliver(delivery) =
		home_prepare(&mut scope, &run, &area, &share_to(HOME, &[&file]))
			.await
			.unwrap()
	else {
		panic!("a new call delivers")
	};
	assert_eq!(delivery.total, 5);
	assert_eq!(delivery.files[0].1, b"hello");
	assert_eq!(delivery.files[0].0.file_id, file.file_id);
}
#[tokio::test]
async fn a_remote_run_sends_to_no_node_but_its_home() {
	let run = home_run();
	let file = hello();
	let area = receiving_area(std::slice::from_ref(&file), &[]);
	let mut scope = receiving(&run);
	for node in ["aidash://other/node", "aidash://home/another"] {
		assert!(matches!(
			home_prepare(&mut scope, &run, &area, &share_to(node, &[&file])).await,
			Err(Error::Forbidden)
		));
	}
	assert_eq!(scope.reads, 0, "nothing is read for a refused target");
	// An Area that no Home started is not served by this path at all.
	let local = Area {
		constraints: json!([]),
		..area
	};
	assert!(matches!(
		home_prepare(&mut scope, &run, &local, &share_to(HOME, &[&file])).await,
		Err(Error::Forbidden)
	));
}
#[tokio::test]
async fn a_reference_set_does_not_stop_the_area_sending_home_but_reference_files_stay() {
	let run = home_run();
	let mut reference = hello();
	reference.path = "docs/spec.md".into();
	reference.scope = FileScope::References;
	reference.provenance = json!({"kind":"reference","id":Uuid::new_v4()});
	let own = {
		let mut made = hello();
		made.path = "made.txt".into();
		made.digest = "a".repeat(64);
		made
	};
	let area = receiving_area(
		&[reference.clone(), own.clone()],
		&[json!({"kind":"reference_bound","id":Uuid::new_v4()})],
	);
	let mut scope = receiving(&run);
	assert!(matches!(
		home_prepare(&mut scope, &run, &area, &share_to(HOME, &[&reference])).await,
		Err(Error::NotFound(_))
	));
	// Its own file passes the Area gate; the stub's bytes do not match its digest, which the
	// Home would never store.
	assert!(matches!(
		home_prepare(&mut scope, &run, &area, &share_to(HOME, &[&own])).await,
		Err(Error::Conflict(code)) if code == "OBJECT_INTEGRITY"
	));
}
#[tokio::test]
async fn private_originals_and_received_files_bar_the_area_even_toward_the_home() {
	let run = home_run();
	let file = hello();
	let mut scope = receiving(&run);
	for kind in ["reference", "reference_text", "received_scope"] {
		let area = receiving_area(std::slice::from_ref(&file), &[json!({"kind":kind})]);
		assert!(matches!(
			home_prepare(&mut scope, &run, &area, &share_to(HOME, &[&file])).await,
			Err(Error::Forbidden)
		));
	}
}
#[tokio::test]
async fn a_repeated_call_is_answered_from_its_stored_result_and_reads_nothing() {
	let run = home_run();
	let file = hello();
	let area = receiving_area(std::slice::from_ref(&file), &[]);
	let mut scope = receiving(&run);
	scope.replay = Some(json!({"status":"completed"}));
	let HomeShare::Replay(previous) =
		home_prepare(&mut scope, &run, &area, &share_to(HOME, &[&file]))
			.await
			.unwrap()
	else {
		panic!("a known call replays")
	};
	assert_eq!(previous["status"], "completed");
	assert_eq!(scope.reads, 0);
}
#[tokio::test]
async fn the_share_budgets_and_the_runs_state_are_held() {
	let run = home_run();
	let file = hello();
	let area = receiving_area(std::slice::from_ref(&file), &[]);
	let mut scope = receiving(&run);
	scope.limits = (4, 4, 200);
	assert!(matches!(
		home_prepare(&mut scope, &run, &area, &share_to(HOME, &[&file])).await,
		Err(Error::Invalid(code)) if code == "SHARE_FILE_LIMIT"
	));
	let mut scope = receiving(&run);
	scope.active = Some(Uuid::new_v4());
	assert!(matches!(
		home_prepare(&mut scope, &run, &area, &share_to(HOME, &[&file])).await,
		Err(Error::Conflict(code)) if code == "RUN_NOT_ACTIVE"
	));
	let mut scope = receiving(&run);
	let mut stale = share_to(HOME, &[&file]);
	stale.expected_revision = 3;
	assert!(matches!(
		home_prepare(&mut scope, &run, &area, &stale).await,
		Err(Error::Conflict(_))
	));
	let mut scope = receiving(&run);
	scope.permitted = false;
	assert!(matches!(
		home_prepare(&mut scope, &run, &area, &share_to(HOME, &[&file])).await,
		Err(Error::Forbidden)
	));
}
