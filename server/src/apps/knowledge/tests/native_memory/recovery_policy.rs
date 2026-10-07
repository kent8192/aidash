//! Managed archives follow current bank settings and retention when reclaiming capacity.
use super::*;
use aidash_server::semantic::services::memory_recovery;

#[rstest]
#[tokio::test]
async fn policy_upgrade_retires_full_archive_capacity_and_shorter_retention(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, registry, workspace) = setup(&database, bounds).await;
	let bank = memory::create_participant(
		&store,
		&Actor::Operator,
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
		&Actor::Operator,
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
	let directory = database.recovery_directory.path();
	let retained = memory_recovery::backup(&store, directory, shared)
		.await
		.unwrap();
	let old = memory_recovery::backup(&store, directory, bank.clone())
		.await
		.unwrap();
	for _ in 0..126 {
		std::fs::copy(&old, old.with_file_name(format!("{}.cbor", Uuid::now_v7()))).unwrap();
	}
	assert!(
		matches!(
			memory_recovery::backup(&store, directory, bank.clone()).await,
			Err(aidash_server::Error::Conflict(_))
		),
		"128 eligible archives enforce capacity"
	);
	let mut provider = registry.get("p", "1.0.0").await.unwrap();
	provider.version = "1.1.0".into();
	provider.config["policy"]["retention"]["backup_days"] = json!(1);
	registry.register(provider).await.unwrap();
	let mut agent = registry.get("a", "1.0.0").await.unwrap();
	agent.version = "1.1.0".into();
	agent.config["memory"] = json!(EntityRef {
		id: "p".into(),
		version: "1.1.0".into()
	});
	registry.register(agent).await.unwrap();
	memory::upgrade_participant(
		&store,
		&Actor::Operator,
		bank.clone(),
		memory::UpgradeParticipant {
			agent: EntityRef {
				id: "a".into(),
				version: "1.1.0".into(),
			},
			expected_revision: 1,
		},
	)
	.await
	.unwrap();
	let fresh = memory_recovery::backup(&store, directory, bank.clone())
		.await
		.unwrap();
	assert!(!old.exists(), "superseded archive revision is retired");
	assert!(
		retained.exists(),
		"another bank's current archive is preserved"
	);
	assert_eq!(
		std::fs::read_dir(directory.join("archives"))
			.unwrap()
			.count(),
		2
	);
	// Fixture time advances beyond current one-day retention while the archive's
	// original expiry is still future; pruning must honor both boundaries.
	let bytes = std::fs::read(&fresh).unwrap();
	let mut encoded: ciborium::value::Value = ciborium::de::from_reader(&bytes[40..]).unwrap();
	let ciborium::value::Value::Map(fields) = &mut encoded else {
		panic!("archive map")
	};
	for (key, value) in fields {
		if key.as_text() == Some("created_at") {
			*value = ciborium::value::Value::Text(
				(chrono::Utc::now() - chrono::Duration::days(2)).to_rfc3339(),
			);
		}
		if key.as_text() == Some("expires_at") {
			*value = ciborium::value::Value::Text(
				(chrono::Utc::now() + chrono::Duration::days(5)).to_rfc3339(),
			);
		}
	}
	write_fixture_archive(&fresh, &encoded);
	memory_recovery::backup(&store, directory, bank)
		.await
		.unwrap();
	assert!(
		!fresh.exists(),
		"current retention expires an archive before original expiry"
	);
	assert!(retained.exists());
	assert_eq!(
		std::fs::read_dir(directory.join("archives"))
			.unwrap()
			.count(),
		2
	);
}
