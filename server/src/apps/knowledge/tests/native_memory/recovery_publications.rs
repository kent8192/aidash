//! Read-only recovery proofs retain the selected publisher's live authority.
use super::*;
use aidash_server::semantic::services::memory_recovery as recovery;

#[rstest]
#[case::credentialless_writer(false)]
#[case::credentialless_publication(true)]
#[tokio::test]
async fn read_only_backup_revalidates_selected_publishers_through_credentialless_lineage(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] nested: bool,
) {
	// Arrange: Alice selects private evidence for disclosure; an Administrator
	// later depends on it, optionally disclosing that dependent a second time.
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let authorization = aidash_server::authorization::Authorization {
		pool: store.pool.clone(),
	};
	let home = aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme");
	let owner = aidash_domain::qualified_agent(&home, "a", "1.0.0");
	authorization
		.replace(
			"acme",
			1,
			serde_json::from_value(json!({
				"tenant": "acme",
				"subjects": {"alice": {"kind": "user"}, owner: {"kind": "agent"}},
				"policies": [{
					"id": "publication-proof",
					"effect": "allow",
					"subjects": {"any": true},
					"actions": ["*"],
					"resources": {"kinds": ["*"]}
				}]
			}))
			.unwrap(),
			"deployment_administrator",
		)
		.await
		.unwrap();
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
			.unwrap();
	}
	let credential = authorization
		.issue_credential("acme", "alice", 3600, "deployment_administrator")
		.await
		.unwrap();
	let actor = authorization.authenticate(&credential.token).await.unwrap();
	let private = memory::create_participant(
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
	let original = memory::mutate(
		&store,
		&actor,
		mutation(
			&private,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("Alice's selected evidence"),
			},
		),
	)
	.await
	.unwrap()
	.remove(0);
	let shared = Bank {
		participant: None,
		..private.clone()
	};
	let selected = publish(&store, &actor, &shared, &original).await;
	let dependent_bank = memory::create_participant(
		&store,
		&Actor::DeploymentAdministrator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let mut dependent = content("Administrator's dependent evidence");
	dependent.evidence = selected.content.evidence.clone();
	let dependent = memory::mutate(
		&store,
		&Actor::DeploymentAdministrator,
		mutation(
			&dependent_bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: dependent,
			},
		),
	)
	.await
	.unwrap()
	.remove(0);
	let bank = if nested {
		// The output body is disposable; its grant still names Alice's original
		// private source. Keep no visible Subject-written unit in this bank so
		// the negative control must reach the nested publisher credential.
		memory::mutate(
			&store,
			&Actor::DeploymentAdministrator,
			mutation(
				&shared,
				Change::Delete {
					id: selected.id,
					expected_revision: selected.revision,
				},
			),
		)
		.await
		.unwrap();
		publish(&store, &Actor::DeploymentAdministrator, &shared, &dependent).await;
		shared
	} else {
		dependent_bank
	};
	let directory = database.recovery_directory.path();
	let job = recovery_history::read_only_job(&store, directory).await;

	// Act/Assert: the controller's actual read-only login can archive these
	// proofs without requiring authority-control or decision INSERT privileges.
	let archive = recovery::backup(&job, directory, bank.clone()).await;
	assert!(
		archive.is_ok(),
		"read-only selected-disclosure proof must not write nested audits: {archive:?}"
	);

	// Suppressing audit writes must never suppress the nested credential check.
	authorization
		.revoke_credential("acme", credential.credential.id)
		.await
		.unwrap();
	let revoked = recovery::backup(&job, directory, bank).await;
	assert!(
		matches!(revoked, Err(aidash_server::Error::Unauthorized)),
		"revoked selected publisher must still withhold the archive: {revoked:?}"
	);
}

async fn publish(store: &Store, actor: &Actor, bank: &Bank, source: &Unit) -> Unit {
	let mutation = mutation(
		bank,
		Change::Add {
			id: Uuid::now_v7(),
			content: source.content.clone(),
		},
	);
	let outcome = memory::operate(
		store,
		actor,
		memory::Operation {
			operation_id: mutation.operation_id,
			provider: reference("p"),
			bank: bank.clone(),
			action: memory::Action::Publish {
				source: source.evidence(),
				mutation,
			},
		},
	)
	.await
	.unwrap();
	let memory::Outcome::Units(mut units) = outcome else {
		panic!("publication must return its admitted unit");
	};
	assert_eq!(units.len(), 1);
	units.remove(0)
}
