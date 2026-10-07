//! Atomic admission preserves storage, provenance and derivation boundaries.
use super::*;

#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn full_bank_replacement_uses_the_final_live_count(
	#[future] database: DatabaseFixture,
	mut bounds: Bounds,
	#[case] add_first: bool,
) {
	bounds.max_units = 2;
	bounds.max_candidates = 2;
	bounds.max_results = 2;
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
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
	let mut original = Vec::new();
	for _ in 0..2 {
		original.extend(
			memory::mutate(
				&store,
				&Actor::Operator,
				mutation(
					&bank,
					Change::Add {
						id: Uuid::now_v7(),
						content: content("Original capacity fixture"),
					},
				),
			)
			.await
			.unwrap(),
		);
	}
	let target = Uuid::now_v7();
	let mut changes = vec![
		Change::Add {
			id: target,
			content: content("Atomic replacement"),
		},
		Change::Delete {
			id: original[0].id,
			expected_revision: 1,
		},
	];
	if !add_first {
		changes.reverse();
	}
	let input = Mutation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: bank.clone(),
		changes,
	};
	let saved = memory::mutate(&store, &Actor::Operator, input.clone())
		.await
		.unwrap();
	assert_eq!(
		memory::mutate(&store, &Actor::Operator, input)
			.await
			.unwrap(),
		saved
	);
	let read = memory::ReadBank {
		provider: reference("p"),
		bank: bank.clone(),
	};
	let current = memory::list(&store, &Actor::Operator, read.clone())
		.await
		.unwrap();
	assert_eq!(current.len(), 2);
	assert!(current.iter().any(|unit| unit.id == target));
	assert!(current.iter().any(|unit| unit.id == original[1].id));
	assert!(
		matches!(memory::mutate(&store, &Actor::Operator, mutation(&bank, Change::Add { id: Uuid::now_v7(), content: content("Still over capacity") })).await, Err(aidash_server::Error::Conflict(message)) if message == "memory bank storage limit reached")
	);
	assert_eq!(
		memory::list(&store, &Actor::Operator, read).await.unwrap(),
		current
	);
}

#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn smaller_provenance_policy_rolls_back_shared_and_participant_changes(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] participant: bool,
) {
	let database = database.await;
	let (store, registry, workspace) = setup(&database, bounds).await;
	let bank = if participant {
		memory::create_participant(
			&store,
			&Actor::Operator,
			workspace,
			memory::CreateParticipant {
				agent: reference("a"),
			},
		)
		.await
		.unwrap()
		.bank
	} else {
		Bank {
			home: store.node_id.clone(),
			tenant: "acme".into(),
			workspace,
			participant: None,
		}
	};
	let mut chain: Vec<Unit> = Vec::new();
	for _ in 0..3 {
		let mut body = content("Current provenance fixture");
		if let Some(parent) = chain.last() {
			body.evidence = vec![parent.evidence()];
		}
		chain.extend(
			memory::mutate(
				&store,
				&Actor::Operator,
				mutation(
					&bank,
					Change::Add {
						id: Uuid::now_v7(),
						content: body,
					},
				),
			)
			.await
			.unwrap(),
		);
	}
	let mut provider = registry.get("p", "1.0.0").await.unwrap();
	provider.id = "small-graph".into();
	provider.config["policy"]["bounds"]["max_graph_visits"] = json!(1);
	registry.register(provider).await.unwrap();
	let result = if participant {
		let mut agent = registry.get("a", "1.0.0").await.unwrap();
		agent.version = "1.1.0".into();
		agent.config["memory"] = json!(reference("small-graph"));
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
		.map(|_| ())
	} else {
		memory::operate(
			&store,
			&Actor::Operator,
			memory::Operation {
				operation_id: Uuid::now_v7(),
				provider: reference("small-graph"),
				bank: bank.clone(),
				action: memory::Action::ConfigureBank {
					expected_revision: 1,
				},
			},
		)
		.await
		.map(|_| ())
	};
	assert!(
		matches!(result, Err(aidash_server::Error::Conflict(message)) if message == "replacement memory policy is below existing provenance")
	);
	let mut current = memory::list(
		&store,
		&Actor::Operator,
		memory::ReadBank {
			provider: reference("p"),
			bank: bank.clone(),
		},
	)
	.await
	.unwrap();
	current.sort_by_key(|unit| unit.id);
	chain.sort_by_key(|unit| unit.id);
	assert_eq!(current, chain);
	let memory::Outcome::Settings(Some(settings)) = memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank: bank.clone(),
			action: memory::Action::Settings,
		},
	)
	.await
	.unwrap() else {
		panic!("retained policy")
	};
	assert_eq!(settings.provider, reference("p"));
	assert_eq!(settings.revision, 1);
	if participant {
		let retained = memory::upgrade_participant(
			&store,
			&Actor::Operator,
			bank,
			memory::UpgradeParticipant {
				agent: reference("a"),
				expected_revision: 1,
			},
		)
		.await
		.unwrap();
		assert_eq!(retained.agent, reference("a"));
	}
}

#[rstest]
#[case(Kind::Observation, false)]
#[case(Kind::Observation, true)]
#[case(Kind::MentalModel, false)]
#[case(Kind::MentalModel, true)]
#[tokio::test]
async fn direct_writes_cannot_create_derived_memory_or_transition_a_base_kind(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] kind: Kind,
	#[case] correction: bool,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
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
	let mut base = Vec::new();
	for _ in 0..2 {
		base.extend(
			memory::mutate(
				&store,
				&Actor::Operator,
				mutation(
					&bank,
					Change::Add {
						id: Uuid::now_v7(),
						content: content("Base memory fixture"),
					},
				),
			)
			.await
			.unwrap(),
		);
	}
	let mut body = content("Forged derived conclusion");
	body.kind = kind;
	body.evidence = vec![base[0].evidence()];
	if kind == Kind::MentalModel {
		body.mental_model = Some(MentalModel {
			question: "Recurring question?".into(),
			automatic_refresh: false,
		});
	}
	let change = if correction {
		Change::Correct {
			id: base[1].id,
			expected_revision: 1,
			content: body,
		}
	} else {
		Change::Add {
			id: Uuid::now_v7(),
			content: body,
		}
	};
	// Even an operator must use the separately validated Derive operation.
	assert!(
		matches!(memory::mutate(&store, &Actor::Operator, mutation(&bank, change)).await, Err(aidash_server::Error::Invalid(message)) if message.contains("derive operation"))
	);
	let mut current = memory::list(
		&store,
		&Actor::Operator,
		memory::ReadBank {
			provider: reference("p"),
			bank,
		},
	)
	.await
	.unwrap();
	base.sort_by_key(|unit| unit.id);
	current.sort_by_key(|unit| unit.id);
	assert_eq!(current, base);
}
