use super::*;
use aidash_server::semantic::services::memory_recovery as management;
use recovery_history::Objects;
use std::sync::atomic::Ordering;

#[rstest]
#[tokio::test]
async fn local_archive_maintenance_reuses_verified_history_during_an_object_outage(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let participant = memory::create_participant(
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
	let bank = Bank {
		participant: None,
		..participant
	};
	memory::mutate(
		&store,
		&administrator(&store).await,
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("locally verified memory"),
			},
		),
	)
	.await
	.unwrap();
	let directory = database.recovery_directory.path();
	let archive = management::backup(&store, directory, bank.clone())
		.await
		.unwrap();
	memory::operate(
		&store,
		&administrator(&store).await,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank: bank.clone(),
			action: memory::Action::ConfigureBank {
				expected_revision: 1,
			},
		},
	)
	.await
	.unwrap();
	let objects = Arc::new(Objects::default());
	let store = management::attach_protected(store, directory.to_owned(), objects.clone())
		.await
		.unwrap();
	let store = management::protect(&store, directory).await.unwrap();
	// Startup established the independent floor. Routine pruning must not replay
	// remote history or hold its exclusive publication lock during an outage.
	objects.outage.store(true, Ordering::SeqCst);
	management::prune_expired(&store, directory)
		.await
		.expect("local archive maintenance must reuse the verified recovery state");
	assert!(
		!archive.exists(),
		"local maintenance must remove the archive retired by its bank revision"
	);
	let units = memory::list(
		&store,
		&administrator(&store).await,
		memory::ReadBank {
			provider: reference("p"),
			bank,
		},
	)
	.await
	.unwrap();
	assert_eq!(
		units.len(),
		1,
		"local maintenance must leave proven reads open"
	);
}
