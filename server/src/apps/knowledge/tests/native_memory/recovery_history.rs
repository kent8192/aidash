use super::*;

#[rstest]
#[tokio::test]
async fn a_protected_home_never_silently_downgrades_to_local_only_memory(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let bank = memory::create_participant(
		&store,
		&administrator(&store).await,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	memory::mutate(
		&store,
		&administrator(&store).await,
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("protected memory"),
			},
		),
	)
	.await
	.unwrap();
	let read = memory::ReadBank {
		provider: reference("p"),
		bank,
	};
	assert_eq!(
		memory::list(&store, &administrator(&store).await, read.clone())
			.await
			.unwrap()
			.len(),
		1
	);
	// This durable marker survives an accidentally removed deployment setting.
	// No GCS credentials or process-global environment are needed for this case.
	std::fs::write(
		database
			.recovery_directory
			.path()
			.join("protection-required"),
		b"1",
	)
	.unwrap();
	assert!(
		memory::list(&store, &administrator(&store).await, read)
			.await
			.is_err(),
		"a protected Home must fail closed when only its local ledger is configured"
	);
	let (closed, _logging) = closure_log();
	management::attach(store, database.recovery_directory.path().to_owned())
		.await
		.unwrap();
	assert!(
		String::from_utf8(closed.lock().unwrap().clone())
			.unwrap()
			.contains("memory_recovery_closed"),
		"a failed protected startup must notify even while ordinary management stays usable"
	);
}

#[derive(Clone)]
struct ClosureLog(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for ClosureLog {
	fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
		self.0.lock().unwrap().extend_from_slice(bytes);
		Ok(bytes.len())
	}
	fn flush(&mut self) -> std::io::Result<()> {
		Ok(())
	}
}
fn closure_log() -> (Arc<Mutex<Vec<u8>>>, tracing::subscriber::DefaultGuard) {
	let bytes = Arc::new(Mutex::new(Vec::new()));
	let captured = bytes.clone();
	let subscriber = tracing_subscriber::fmt()
		.with_max_level(tracing::Level::ERROR)
		.with_ansi(false)
		.with_writer(move || ClosureLog(captured.clone()))
		.finish();
	(bytes, tracing::subscriber::set_default(subscriber))
}

use aidash_application::ports::memory::objects::{Object, ObjectName, ObjectPage, RecoveryObjects};
use aidash_application::{Error as ApplicationError, Result as ApplicationResult};
use aidash_server::semantic::services::memory_recovery as management;
use std::{collections::BTreeMap, sync::atomic::Ordering};

/// Models object-store durability separately from delivery of its acknowledgement.
#[derive(Default)]
pub(super) struct Objects {
	entries: Mutex<BTreeMap<String, Object>>,
	pub(super) outage: AtomicBool,
	lose_reply: AtomicBool,
	read_only_shards: Mutex<Option<std::path::PathBuf>>,
	pause: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
	pub(super) pause_list: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}
#[async_trait::async_trait]
impl RecoveryObjects for Objects {
	async fn get(&self, name: &str, generation: Option<&str>) -> ApplicationResult<Option<Object>> {
		if self.outage.load(Ordering::SeqCst) {
			return Err(ApplicationError::SemanticUnavailable);
		}
		let entries = self.entries.lock().unwrap();
		Ok(entries
			.get(name)
			.filter(|value| generation.is_none_or(|g| g == value.generation))
			.cloned())
	}
	async fn create(&self, name: &str, bytes: &[u8]) -> ApplicationResult<String> {
		let pause = self.pause.lock().unwrap().take();
		if let Some((entered, release)) = pause {
			entered.notify_one();
			release.notified().await;
		}
		if self.outage.load(Ordering::SeqCst) {
			return Err(ApplicationError::SemanticUnavailable);
		}
		let mut entries = self.entries.lock().unwrap();
		if let Some(old) = entries.get(name) {
			return if old.bytes == bytes {
				Ok(old.generation.clone())
			} else {
				Err(ApplicationError::Conflict(
					"immutable object differs".into(),
				))
			};
		}
		let generation = (entries.len() + 1).to_string();
		entries.insert(
			name.to_owned(),
			Object {
				bytes: bytes.to_vec(),
				generation: generation.clone(),
			},
		);
		if let Some(path) = self.read_only_shards.lock().unwrap().take() {
			use std::os::unix::fs::PermissionsExt;
			std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o500)).unwrap();
		}
		if self.lose_reply.swap(false, Ordering::SeqCst) {
			return Err(ApplicationError::SemanticUnavailable);
		}
		Ok(generation)
	}
	async fn list(&self, prefix: &str, _: Option<&str>) -> ApplicationResult<ObjectPage> {
		let pause = self.pause_list.lock().unwrap().take();
		if let Some((entered, release)) = pause {
			entered.notify_one();
			release.notified().await;
		}
		if self.outage.load(Ordering::SeqCst) {
			return Err(ApplicationError::SemanticUnavailable);
		}
		Ok(ObjectPage {
			objects: self
				.entries
				.lock()
				.unwrap()
				.iter()
				.filter(|(key, _)| key.starts_with(prefix))
				.map(|(name, value)| ObjectName {
					name: name.clone(),
					generation: value.generation.clone(),
				})
				.collect(),
			next: None,
		})
	}
	async fn delete(&self, name: &str, generation: &str) -> ApplicationResult<()> {
		if self.outage.load(Ordering::SeqCst) {
			return Err(ApplicationError::SemanticUnavailable);
		}
		let mut entries = self.entries.lock().unwrap();
		if entries
			.get(name)
			.is_some_and(|old| old.generation != generation)
		{
			return Err(ApplicationError::Conflict("generation changed".into()));
		}
		entries.remove(name);
		Ok(())
	}
}

#[rstest]
#[case("lost_reply")]
#[case("local_write")]
#[tokio::test]
async fn uncertain_floor_publication_stays_closed_across_restart_and_preserves_the_floor(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] boundary: &str,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let bank = memory::create_participant(
		&store,
		&administrator(&store).await,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let id = Uuid::now_v7();
	memory::mutate(
		&store,
		&administrator(&store).await,
		mutation(
			&bank,
			Change::Add {
				id,
				content: content("original"),
			},
		),
	)
	.await
	.unwrap();
	let objects = Arc::new(Objects::default());
	let directory = database.recovery_directory.path();
	let store = management::attach_protected(store, directory.to_owned(), objects.clone())
		.await
		.unwrap();
	let store = management::protect(&store, directory).await.unwrap();
	let read = memory::ReadBank {
		provider: reference("p"),
		bank: bank.clone(),
	};
	assert_eq!(
		memory::list(&store, &administrator(&store).await, read.clone())
			.await
			.unwrap()
			.len(),
		1
	);
	let (closed, _logging) = closure_log();
	if boundary == "lost_reply" {
		objects.lose_reply.store(true, Ordering::SeqCst);
	} else {
		// After durable append, make the actual shard directory unwritable.
		*objects.read_only_shards.lock().unwrap() = Some(directory.join("units"));
	}
	let result = memory::mutate(
		&store,
		&administrator(&store).await,
		mutation(
			&bank,
			Change::Correct {
				id,
				expected_revision: 1,
				content: content("uncertain revision"),
			},
		),
	)
	.await;
	assert!(result.is_err());
	let notification = String::from_utf8(closed.lock().unwrap().clone()).unwrap();
	assert!(
		notification.contains("memory_recovery_closed"),
		"unknown publication must notify immediately: {notification}"
	);
	assert!(!notification.contains("uncertain revision"));
	assert!(directory.join("publication-pending.cbor").exists());
	assert!(
		memory::list(&store, &administrator(&store).await, read.clone())
			.await
			.is_err()
	);
	if boundary == "local_write" {
		use std::os::unix::fs::PermissionsExt;
		std::fs::set_permissions(
			directory.join("units"),
			std::fs::Permissions::from_mode(0o700),
		)
		.unwrap();
	}
	let restarted = management::attach_protected(store, directory.to_owned(), objects)
		.await
		.unwrap();
	assert!(!directory.join("publication-pending.cbor").exists());
	// SQL rolled back to revision one; the durable attempted revision two is never rewound.
	assert!(
		memory::list(&restarted, &administrator(&restarted).await, read)
			.await
			.is_err()
	);
	assert!(
		memory::mutate(
			&restarted,
			&administrator(&restarted).await,
			mutation(
				&bank,
				Change::Correct {
					id,
					expected_revision: 1,
					content: content("different second revision")
				}
			)
		)
		.await
		.is_err()
	);
}

#[rstest]
#[tokio::test]
async fn an_unavailable_preflight_rejects_writes_without_closing_proven_reads(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let bank = memory::create_participant(
		&store,
		&administrator(&store).await,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let id = Uuid::now_v7();
	memory::mutate(
		&store,
		&administrator(&store).await,
		mutation(
			&bank,
			Change::Add {
				id,
				content: content("proven local memory"),
			},
		),
	)
	.await
	.unwrap();
	let objects = Arc::new(Objects::default());
	let directory = database.recovery_directory.path();
	let store = management::attach_protected(store, directory.to_owned(), objects.clone())
		.await
		.unwrap();
	let store = management::protect(&store, directory).await.unwrap();
	objects.outage.store(true, Ordering::SeqCst);
	assert!(
		memory::mutate(
			&store,
			&administrator(&store).await,
			mutation(
				&bank,
				Change::Correct {
					id,
					expected_revision: 1,
					content: content("must not commit")
				}
			)
		)
		.await
		.is_err()
	);
	assert!(!directory.join("publication-pending.cbor").exists());
	let read = memory::ReadBank {
		provider: reference("p"),
		bank,
	};
	assert_eq!(
		memory::list(&store, &administrator(&store).await, read)
			.await
			.unwrap()[0]
			.content
			.text,
		"proven local memory"
	);
}

#[rstest]
#[case(false, false)]
#[case(true, false)]
#[case(true, true)]
#[tokio::test]
async fn a_failed_sql_commit_is_preserved_in_independent_history(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] deleting: bool,
	#[case] hidden: bool,
) {
	use aidash_server::database::native;
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let bank = memory::create_participant(
		&store,
		&administrator(&store).await,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let id = Uuid::now_v7();
	memory::mutate(
		&store,
		&administrator(&store).await,
		mutation(
			&bank,
			Change::Add {
				id,
				content: {
					let mut content = content("before deferred failure");
					if hidden {
						content.verification = Verification::Contradicted;
					}
					content
				},
			},
		),
	)
	.await
	.unwrap();
	let objects = Arc::new(Objects::default());
	let directory = database.recovery_directory.path();
	let store = management::attach_protected(store, directory.to_owned(), objects.clone())
		.await
		.unwrap();
	let store = management::protect(&store, directory).await.unwrap();
	// DDL exception: Reinhardt Query cannot express deferred constraint triggers.
	// This rejects COMMIT after the actual floor publication, not the row update.
	let owner: native::Pool = crate::native_database::owner_pool(store.pool.fixture_driver())
		.await
		.into();
	let mut tx = owner.begin(&TEST_FIXTURE).await.unwrap();
	for ddl in [
		"CREATE FUNCTION reject_memory_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture memory commit failure'; END $$",
		"CREATE CONSTRAINT TRIGGER reject_memory_commit AFTER UPDATE ON memory_units DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_memory_commit()",
	] {
		native::query(ddl).execute(&mut *tx).await.unwrap();
	}
	tx.commit().await.unwrap();
	assert!(
		memory::mutate(
			&store,
			&administrator(&store).await,
			mutation(
				&bank,
				if deleting {
					Change::Delete {
						id,
						expected_revision: 1,
					}
				} else {
					Change::Correct {
						id,
						expected_revision: 1,
						content: content("durable attempted correction"),
					}
				}
			)
		)
		.await
		.is_err()
	);
	assert!(!directory.join("publication-pending.cbor").exists());
	let record: aidash_domain::memory::recovery::history::Record = {
		let remote = objects.entries.lock().unwrap();
		let record = remote
			.iter()
			.find(|(name, _)| name.contains("/records/"))
			.unwrap()
			.1;
		serde_json::from_slice(&record.bytes).unwrap()
	};
	let aidash_domain::memory::recovery::history::Change::Floors { units, .. } = record.change
	else {
		panic!("floor record");
	};
	assert_eq!(units[0].fence.revision, 2);
	assert_eq!(units[0].fence.deleted, deleting);
	// Total local loss does not discard the attempted floor even though SQL rolled back.
	std::fs::remove_dir_all(directory).unwrap();
	let head = management::reconstruct(&store, directory).await.unwrap();
	assert!(head.recovering);
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	let revision: i64 = native::query_scalar(
		&Query::select()
			.column(Alias::new("revision"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut *tx)
	.await
	.unwrap();
	assert_eq!(revision, 1);
	tx.rollback().await.unwrap();
	let mut tx = owner.begin(&TEST_FIXTURE).await.unwrap();
	native::query("DROP TRIGGER reject_memory_commit ON memory_units")
		.execute(&mut *tx)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	let empty_archives = Arc::new(Objects::default());
	let report = management::protection::reconcile_all(&store, directory, empty_archives.clone())
		.await
		.unwrap();
	assert_eq!((report.restored, report.withheld), (0, 1));
	management::protection::resume(&store, directory, empty_archives, &report)
		.await
		.unwrap();
	let negative: (i64, bool, String) = native::query_as(
		&Query::select()
			.columns(["revision", "deleted", "text"].map(Alias::new))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["revision", "deleted", "text"])
	.fetch_one(store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	assert_eq!(
		negative,
		(if deleting { 2 } else { 3 }, deleting, String::new()),
		"a permanent negative receipt keeps its original revision while the rolled-back body is cleared"
	);
	let restarted =
		management::attach_protected(store.clone(), directory.to_owned(), objects.clone())
			.await
			.unwrap();
	let read = memory::ReadBank {
		provider: reference("p"),
		bank,
	};
	assert!(
		memory::list(&restarted, &administrator(&restarted).await, read)
			.await
			.unwrap()
			.is_empty()
	);
}

#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn lost_ledger_recovery_uses_current_bodies_and_never_revives_an_old_archive(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] copied_archive: bool,
	#[future(awt)]
	#[from(memory_restore_rebuilds_missing_vectors_and_checks_open_epoch_reads_provider)]
	fixture: MemoryRestoreRebuildsMissingVectorsAndChecksOpenEpochReadsProvider,
) {
	let database = database.await;
	let server = fixture.server;
	let endpoint = format!("{}/v1", server.url);
	let (store, _, workspace) = setup_endpoint(&database, bounds, &endpoint).await;
	let bank = memory::create_participant(
		&store,
		&administrator(&store).await,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let id = Uuid::now_v7();
	let keep = Uuid::now_v7();
	for (id, text) in [
		(id, "withdrawn after archive"),
		(keep, "current body survives disk loss"),
	] {
		memory::mutate(
			&store,
			&administrator(&store).await,
			mutation(
				&bank,
				Change::Add {
					id,
					content: content(text),
				},
			),
		)
		.await
		.unwrap();
	}
	let history = Arc::new(Objects::default());
	let archives = Arc::new(Objects::default());
	let directory = database.recovery_directory.path();
	let store = management::attach_protected(store, directory.to_owned(), history.clone())
		.await
		.unwrap();
	let store = management::protect(&store, directory).await.unwrap();
	if copied_archive {
		let receipt = management::protection::backup_all(
			&store,
			directory,
			archives.clone(),
			Default::default(),
		)
		.await
		.unwrap();
		assert!(receipt.complete);
	}
	memory::mutate(
		&store,
		&administrator(&store).await,
		mutation(
			&bank,
			Change::Delete {
				id,
				expected_revision: 1,
			},
		),
	)
	.await
	.unwrap();
	std::fs::remove_dir_all(directory).unwrap();
	let barrier = management::reconstruct(&store, directory).await.unwrap();
	assert!(barrier.recovering);
	let store = management::attach_protected(store, directory.to_owned(), history.clone())
		.await
		.unwrap();
	let read = memory::ReadBank {
		provider: reference("p"),
		bank,
	};
	assert!(
		memory::list(&store, &administrator(&store).await, read.clone())
			.await
			.is_err()
	);
	if !copied_archive {
		archives.outage.store(true, Ordering::SeqCst);
	}
	let result = management::protection::reconcile_all(&store, directory, archives.clone()).await;
	if let Err(error) = &result {
		let entries: Vec<(String, Option<String>)> = aidash_server::database::native::query_as(
			&Query::select()
				.columns(["state", "last_error"].map(Alias::new))
				.from(Alias::new("semantic_entries"))
				.to_string(PostgresQueryBuilder),
		)
		.columns(&["state", "last_error"])
		.fetch_all(store.pool.in_scope(&TEST_FIXTURE))
		.await
		.unwrap();
		panic!("reconciliation failed: {error:?}; indexing receipts: {entries:?}");
	}
	let report = result.unwrap();
	assert_eq!(report.restored, 1);
	assert!(report.head.recovering);
	// Resume commits remotely, but the local reconstruction stays closed until
	// an exact retry confirms the lost acknowledgement and finishes the handoff.
	history.lose_reply.store(true, Ordering::SeqCst);
	assert!(
		management::protection::resume(&store, directory, archives.clone(), &report)
			.await
			.is_err()
	);
	assert!(
		memory::list(&store, &administrator(&store).await, read.clone())
			.await
			.is_err()
	);
	management::protection::resume(&store, directory, archives.clone(), &report)
		.await
		.unwrap();
	let store = management::attach_protected(store, directory.to_owned(), history)
		.await
		.unwrap();
	let served = memory::list(&store, &administrator(&store).await, read.clone())
		.await
		.unwrap();
	assert_eq!(served.len(), 1);
	assert_eq!(served[0].id, keep);
	assert_eq!(served[0].content.text, "current body survives disk loss");
	// A completed report cannot reopen a later ordinary restore, even when the
	// immutable head still points to that report's successful Resume record.
	management::prepare_restore(&store, directory)
		.await
		.unwrap();
	assert!(matches!(
		management::protection::resume(&store, directory, archives, &report).await,
		Err(aidash_server::Error::Conflict(_))
	));
	assert!(
		memory::list(&store, &administrator(&store).await, read)
			.await
			.is_err()
	);
	drop(server);
}

#[rstest]
#[tokio::test]
async fn all_bank_archives_continue_after_a_failure_and_prune_without_the_ledger(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use aidash_server::database::native;
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let bank = memory::create_participant(
		&store,
		&administrator(&store).await,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let shared = Bank {
		participant: None,
		..bank.clone()
	};
	memory::operate(
		&store,
		&administrator(&store).await,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank: shared.clone(),
			action: memory::Action::ConfigureBank {
				expected_revision: 0,
			},
		},
	)
	.await
	.unwrap();
	let remote = Arc::new(Objects::default());
	remote.lose_reply.store(true, Ordering::SeqCst);
	let directory = database.recovery_directory.path();
	let receipt =
		management::protection::backup_all(&store, directory, remote.clone(), Default::default())
			.await
			.unwrap();
	assert!(!receipt.complete);
	assert_eq!(
		receipt
			.banks
			.iter()
			.filter(|bank| bank.state == "failed")
			.count(),
		1
	);
	assert_eq!(
		receipt
			.banks
			.iter()
			.filter(|bank| bank.state == "verified")
			.count(),
		1
	);
	assert_eq!(
		std::fs::read_dir(directory.join("spool")).unwrap().count(),
		1,
		"lost upload reply retains its valid spool file"
	);
	assert!(
		management::protection::backup_all(&store, directory, remote.clone(), Default::default())
			.await
			.unwrap()
			.complete
	);
	assert_eq!(
		std::fs::read_dir(directory.join("spool")).unwrap().count(),
		0
	);
	// Changing the exact settings revision retires that bank's copies. A malformed
	// copy is independently deleted, and losing the ledger cannot stop either.
	memory::operate(
		&store,
		&administrator(&store).await,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank: shared,
			action: memory::Action::ConfigureBank {
				expected_revision: 1,
			},
		},
	)
	.await
	.unwrap();
	let prefix = format!(
		"homes/{}/archives/",
		aidash_domain::semantic::indexing::content_digest(store.deployment.as_str())
	);
	remote
		.create(&format!("{prefix}malformed.cbor"), b"truncated")
		.await
		.unwrap();
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_bank_settings"))
			.value(
				Alias::new("recovery_eligible_at"),
				chrono::Utc::now() - chrono::Duration::hours(9),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	std::fs::remove_dir_all(directory).unwrap();
	let result = management::protection::prune_remote(&store, remote)
		.await
		.unwrap();
	assert!(result.complete);
	assert!(result.deleted >= 2);
	assert_eq!(
		result.stale.len(),
		1,
		"a newly configured revision has no valid copy and its persisted eligibility clock is overdue"
	);
}

#[rstest]
#[tokio::test]
async fn a_delayed_old_writer_cannot_publish_after_the_replacement_disk_barrier(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let bank = memory::create_participant(
		&store,
		&administrator(&store).await,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let history = Arc::new(Objects::default());
	let directory = database.recovery_directory.path();
	let store = management::attach_protected(store, directory.to_owned(), history.clone())
		.await
		.unwrap();
	let store = management::protect(&store, directory).await.unwrap();
	let entered = Arc::new(tokio::sync::Notify::new());
	let release = Arc::new(tokio::sync::Notify::new());
	*history.pause.lock().unwrap() = Some((entered.clone(), release.clone()));
	let writer = store.clone();
	let change = mutation(
		&bank,
		Change::Add {
			id: Uuid::now_v7(),
			content: content("late old generation"),
		},
	);
	let task = tokio::spawn(async move {
		memory::mutate(&writer, &administrator(&writer).await, change).await
	});
	tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
		.await
		.unwrap();
	assert!(directory.join("publication-pending.cbor").exists());
	// Physical loss unlinks the old lock file. Reconstruction claims the next
	// immutable sequence slot before the outstanding request reaches GCS.
	std::fs::remove_dir_all(directory).unwrap();
	let barrier = management::reconstruct(&store, directory).await.unwrap();
	assert_eq!(barrier.sequence, 1);
	release.notify_one();
	assert!(task.await.unwrap().is_err());
	let values = history.entries.lock().unwrap();
	let bytes = &values
		.iter()
		.find(|(name, _)| name.contains("/records/"))
		.unwrap()
		.1
		.bytes;
	let record: aidash_domain::memory::recovery::history::Record =
		serde_json::from_slice(bytes).unwrap();
	assert!(matches!(
		record.change,
		aidash_domain::memory::recovery::history::Change::Barrier
	));
	assert_eq!(
		values
			.keys()
			.filter(|name| name.contains("/records/"))
			.count(),
		1
	);
}

#[rstest]
#[tokio::test]
async fn lost_ledger_reconciliation_fills_a_matching_missing_body_from_its_off_disk_copy(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[future(awt)]
	#[from(memory_restore_rebuilds_missing_vectors_and_checks_open_epoch_reads_provider)]
	fixture: MemoryRestoreRebuildsMissingVectorsAndChecksOpenEpochReadsProvider,
) {
	use aidash_server::database::native;
	let database = database.await;
	let server = fixture.server;
	let endpoint = format!("{}/v1", server.url);
	let (store, _, workspace) = setup_endpoint(&database, bounds, &endpoint).await;
	let bank = memory::create_participant(
		&store,
		&administrator(&store).await,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let id = Uuid::now_v7();
	memory::mutate(
		&store,
		&administrator(&store).await,
		mutation(
			&bank,
			Change::Add {
				id,
				content: content("matching archived body"),
			},
		),
	)
	.await
	.unwrap();
	let history = Arc::new(Objects::default());
	let remote = Arc::new(Objects::default());
	let directory = database.recovery_directory.path();
	let store = management::attach_protected(store, directory.to_owned(), history.clone())
		.await
		.unwrap();
	let store = management::protect(&store, directory).await.unwrap();
	assert!(
		management::protection::backup_all(&store, directory, remote.clone(), Default::default())
			.await
			.unwrap()
			.complete
	);
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_units"))
			.value(Alias::new("text"), "")
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	std::fs::remove_dir_all(directory).unwrap();
	management::reconstruct(&store, directory).await.unwrap();
	let report = management::protection::reconcile_all(&store, directory, remote.clone())
		.await
		.unwrap();
	assert_eq!(report.restored, 1);
	management::protection::resume(&store, directory, remote, &report)
		.await
		.unwrap();
	let store = management::attach_protected(store, directory.to_owned(), history)
		.await
		.unwrap();
	let units = memory::list(
		&store,
		&administrator(&store).await,
		memory::ReadBank {
			bank,
			provider: reference("p"),
		},
	)
	.await
	.unwrap();
	assert_eq!(units[0].content.text, "matching archived body");
	drop(server);
}

#[rstest]
#[tokio::test]
async fn interrupted_protection_seeding_can_finish_without_reopening_an_unrelated_restore(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let bank = memory::create_participant(
		&store,
		&administrator(&store).await,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	memory::mutate(
		&store,
		&administrator(&store).await,
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("complete seed includes this floor"),
			},
		),
	)
	.await
	.unwrap();
	let history = Arc::new(Objects::default());
	let directory = database.recovery_directory.path();
	let store = management::attach_protected(store, directory.to_owned(), history.clone())
		.await
		.unwrap();
	history.lose_reply.store(true, Ordering::SeqCst);
	assert!(management::protect(&store, directory).await.is_err());
	let read = memory::ReadBank {
		provider: reference("p"),
		bank,
	};
	assert!(
		memory::list(&store, &administrator(&store).await, read.clone())
			.await
			.is_err()
	);
	let protected = management::protect(&store, directory).await.unwrap();
	assert_eq!(
		memory::list(&protected, &administrator(&protected).await, read.clone())
			.await
			.unwrap()
			.len(),
		1
	);
	// Re-running activation of an already protected Home cannot open an explicit restore gate.
	management::prepare_restore(&protected, directory)
		.await
		.unwrap();
	let protected = management::protect(&protected, directory).await.unwrap();
	assert!(
		memory::list(&protected, &administrator(&protected).await, read)
			.await
			.is_err()
	);
}

#[rstest]
#[tokio::test]
async fn a_killed_archive_temporary_cannot_bypass_spool_bounds_or_block_policy_expiry(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let bank = Bank {
		home: aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme")
			.to_string(),
		tenant: "acme".into(),
		workspace,
		participant: None,
	};
	memory::operate(
		&store,
		&administrator(&store).await,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank: bank.clone(),
			action: memory::Action::ConfigureBank {
				expected_revision: 0,
			},
		},
	)
	.await
	.unwrap();
	let directory = database.recovery_directory.path();
	let spool = directory.join("spool");
	std::fs::create_dir(&spool).unwrap();
	let abandoned = spool.join(format!(".memory-{}.pending", Uuid::new_v4()));
	std::fs::File::create(&abandoned)
		.unwrap()
		.set_len(64 * 1024 * 1024)
		.unwrap();
	let remote = Arc::new(Objects::default());
	remote.outage.store(true, Ordering::SeqCst);
	let limits = management::protection::SpoolLimits {
		files: 1,
		bytes: 64 * 1024 * 1024,
	};
	assert!(
		!management::protection::backup_all(&store, directory, remote.clone(), limits)
			.await
			.unwrap()
			.complete
	);
	assert!(
		!abandoned.exists(),
		"an uncommitted archive temporary is reclaimed under the spool lock"
	);
	let old = std::fs::read_dir(&spool)
		.unwrap()
		.next()
		.unwrap()
		.unwrap()
		.path();
	let bytes = std::fs::read(&old).unwrap();
	assert!(
		!management::protection::backup_all(&store, directory, remote.clone(), limits)
			.await
			.unwrap()
			.complete
	);
	assert_eq!(
		std::fs::read(&old).unwrap(),
		bytes,
		"valid unsent copies are preserved"
	);
	memory::operate(
		&store,
		&administrator(&store).await,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank,
			action: memory::Action::ConfigureBank {
				expected_revision: 1,
			},
		},
	)
	.await
	.unwrap();
	assert!(
		!management::protection::backup_all(&store, directory, remote.clone(), limits)
			.await
			.unwrap()
			.complete
	);
	assert!(
		!old.exists(),
		"policy-invalid unsent copies expire even while GCS is unavailable"
	);
	let unknown = spool.join("retained-diagnostic");
	std::fs::write(&unknown, b"unknown files also consume the bounded spool").unwrap();
	remote.outage.store(false, Ordering::SeqCst);
	assert!(
		!management::protection::backup_all(&store, directory, remote, limits)
			.await
			.unwrap()
			.complete,
		"unknown entries cannot bypass spool capacity accounting"
	);
	assert!(
		unknown.exists(),
		"only recognized abandoned temporaries are reclaimed automatically"
	);
}

#[rstest]
#[tokio::test]
async fn scheduled_backup_revalidates_subject_authority_with_the_actual_read_only_job_login(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let authorization = aidash_server::authorization::Authorization {
		pool: store.pool.clone(),
	};
	let home = aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme");
	let owner = aidash_domain::qualified_agent(&home, "a", "1.0.0");
	authorization.replace("acme", 1, serde_json::from_value(json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},owner:{"kind":"agent","node":&home.node}},"policies":[{"id":"backup-proof","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]})).unwrap(), "deployment_administrator").await.unwrap();
	for id in ["m", "e", "r", "t", "p", "a"] {
		authorization
			.set_catalog(
				"acme",
				&home.node,
				&reference(id),
				0,
				true,
				"deployment_administrator",
			)
			.await
			// The fixture Subject may have approved it already.
			.ok();
	}
	let credential = authorization
		.issue_credential("acme", "alice", 3600, "deployment_administrator")
		.await
		.unwrap();
	let actor = authorization.authenticate(&credential.token).await.unwrap();
	let bank = memory::create_participant(
		&store,
		&actor,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	memory::mutate(
		&store,
		&actor,
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("Subject-owned backup proof"),
			},
		),
	)
	.await
	.unwrap();
	let job = read_only_job(&store, database.recovery_directory.path()).await;
	let receipt = management::protection::backup_all(
		&job,
		database.recovery_directory.path(),
		Arc::new(Objects::default()),
		Default::default(),
	)
	.await
	.unwrap();
	assert!(
		receipt.complete,
		"Subject-origin authority checks must not require audit writes under the read-only Job login"
	);
	// Start the actual command with only the maintenance credentials and public
	// settings, as a CronJob does. No serving Node token or session secret is inherited.
	use sqlx::ConnectOptions as _;
	let settings = tempfile::tempdir().unwrap();
	std::fs::write(
		settings.path().join("base.toml"),
		include_str!(concat!(
			env!("CARGO_MANIFEST_DIR"),
			"/settings/base.example.toml"
		)),
	)
	.unwrap();
	let output = std::process::Command::new(env!("CARGO_BIN_EXE_manage"))
		.args(["memory-recovery", "status", "--directory"])
		.arg(database.recovery_directory.path())
		.arg("--json")
		.env_clear()
		.env("REINHARDT_ENV", "local")
		.env("REINHARDT_SETTINGS_DIR", settings.path())
		.env(
			"DATABASE_URL",
			job.pool.connect_options().to_url_lossy().as_str(),
		)
		.env("AIDASH_DEPLOYMENT_ID", job.deployment.as_str())
		.env(
			"AIDASH_API_TOKEN",
			"isolated-maintenance-validation-token-with-no-server-authority",
		)
		.output()
		.unwrap();
	assert!(
		output.status.success(),
		"maintenance command startup: {}",
		String::from_utf8_lossy(&output.stderr)
	);
	let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
	assert_eq!(status["serving"], true);
}

/// Real controller-owned database boundary shared by Subject and Run-origin archive tests.
pub(super) async fn read_only_job(store: &Store, directory: &std::path::Path) -> Store {
	use sqlx::Executor as _;
	// Exercise the exact controller-managed login/guards, not a test approximation.
	// DDL exception: role grants and trigger/function definitions are not Query operations.
	let ddl = std::process::Command::new("python3").current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())
		.args(["-c", "import sys; sys.path.insert(0, 'infra/gcp/control'); from kube import memory_role_sql; print(memory_role_sql('3' * 64))"])
		.output().unwrap();
	assert!(ddl.status.success());
	let ddl = String::from_utf8(ddl.stdout).unwrap().replace(
		"GRANT CONNECT ON DATABASE aidash_a",
		&format!(
			"GRANT CONNECT ON DATABASE {}",
			store.pool.connect_options().get_database().unwrap()
		),
	);
	let owner_pool = crate::native_database::admin_pool(store.pool.fixture_driver()).await;
	owner_pool.execute(ddl.as_str()).await.unwrap();
	let options = store
		.pool
		.connect_options()
		.as_ref()
		.clone()
		.username("aidash_memory_recovery")
		.password(&"3".repeat(64));
	let pool = sqlx::postgres::PgPoolOptions::new()
		.max_connections(8)
		.connect_with(options)
		.await
		.unwrap();
	let mut job = Store::from_pool(pool, store.deployment.clone())
		.await
		.unwrap();
	job.keyless = store.keyless.clone();
	management::attach(job, directory.to_owned()).await.unwrap()
}
