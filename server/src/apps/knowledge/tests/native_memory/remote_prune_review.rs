use super::*;
use aidash_application::ports::memory::objects::RecoveryObjects;
use aidash_server::semantic::services::memory_recovery as management;
use recovery_history::Objects;

#[rstest]
#[tokio::test]
async fn remote_prune_preserves_archives_created_after_its_cut(
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
	let remote = Arc::new(Objects::default());
	let prefix = format!(
		"homes/{}/archives/",
		aidash_domain::semantic::indexing::content_digest(store.deployment.as_str())
	);
	let entered = Arc::new(tokio::sync::Notify::new());
	let release = Arc::new(tokio::sync::Notify::new());
	*remote.pause_list.lock().unwrap() = Some((entered.clone(), release.clone()));
	let pruning = management::protection::prune_remote(&store, remote.clone());
	let backup = async {
		// The listing begins only after prune has captured its cut and read the
		// bank inventory. Generate a real archive while that listing is pending.
		entered.notified().await;
		let report = management::protection::backup_all(
			&store,
			database.recovery_directory.path(),
			remote.clone(),
			Default::default(),
		)
		.await
		.unwrap();
		release.notify_one();
		report
	};
	let (pruned, backed_up) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
		tokio::join!(pruning, backup)
	})
	.await
	.expect("overlapping backup and prune must finish");
	assert!(backed_up.complete);
	assert_eq!(backed_up.banks.len(), 1);
	let pruned = pruned.unwrap();
	assert!(pruned.complete);
	assert_eq!(
		pruned.deleted, 0,
		"a later archive lies outside this prune cut"
	);
	let retained = remote.list(&prefix, None).await.unwrap().objects;
	assert_eq!(retained.len(), 1, "the newly verified backup must survive");
	// Deferring the concurrent archive is bounded by the current prune pass.
	// A later pass must still remove it when the live policy retires its revision.
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
	let retired = management::protection::prune_remote(&store, remote.clone())
		.await
		.unwrap();
	assert!(retired.complete);
	assert_eq!(retired.deleted, 1);
	assert!(remote.list(&prefix, None).await.unwrap().objects.is_empty());
}
