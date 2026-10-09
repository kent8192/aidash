use super::*;

#[test]
fn full_workspace_context_retains_content_with_minimal_memory_status() {
	let budget = 256;
	let available = workspace_budget(budget).unwrap();
	let semantic = json!("x".repeat(available - 2));
	assert_eq!(serde_json::to_vec(&semantic).unwrap().len(), available);
	let reserved = serde_json::to_vec(&json!({"workspace":semantic,"memory":null}))
		.unwrap()
		.len();
	for status in ["disabled", "no_space"] {
		let memory = bounded_status(status, budget - reserved).unwrap();
		let combined = combine(Some(semantic.clone()), memory, budget)
			.unwrap()
			.unwrap();
		assert_eq!(combined["workspace"], semantic);
		assert_eq!(combined["memory"]["status"], status);
		assert!(serde_json::to_vec(&combined).unwrap().len() <= budget);
	}
}

#[test]
fn duplicate_source_caps_restrict_private_and_shared_banks_in_any_order() {
	let private = Bank {
		home: "aidash://home".into(),
		tenant: "acme".into(),
		workspace: uuid::Uuid::now_v7(),
		participant: Some(uuid::Uuid::now_v7()),
	};
	let provider = EntityRef {
		id: "memory".into(),
		version: "1.0.0".into(),
	};
	for initial in [2048, usize::MAX] {
		for caps in [[80, 20], [20, 80]] {
			let mut declared = vec![(private.clone(), provider.clone(), initial)];
			let mut shared = private.clone();
			shared.participant = None;
			for cap in caps {
				declare_bank(&mut declared, private.clone(), provider.clone(), cap);
				declare_bank(&mut declared, shared.clone(), provider.clone(), cap / 2);
			}
			assert_eq!(
				declared,
				vec![
					(private.clone(), provider.clone(), 20),
					(shared, provider.clone(), 10)
				]
			);
			let other = EntityRef {
				id: "other".into(),
				version: "1.0.0".into(),
			};
			declare_bank(&mut declared, private.clone(), other.clone(), 50);
			assert_eq!(declared.len(), 3);
			assert_eq!(declared[2], (private.clone(), other, 50));
		}
	}
}

#[test]
fn ordered_envelope_omits_step_and_run_revision_but_the_operation_keeps_them() {
	let inputs = vec![(
		InputRead {
			id: uuid::Uuid::now_v7(),
			sequence: 4,
			digest: "sha256:input".into(),
		},
		"input".to_owned(),
	)];
	let (legacy_operation, legacy_visible) =
		boundaries(3, &inputs, 2, 9, ProjectionVersion::Legacy);
	assert_eq!(legacy_visible, legacy_operation);
	assert_eq!(legacy_visible["step"], json!(2));
	assert_eq!(legacy_visible["run_revision"], json!(9));

	let (first_operation, first) = boundaries(3, &inputs, 2, 9, ProjectionVersion::Ordered);
	let (later_operation, later) = boundaries(3, &inputs, 5, 14, ProjectionVersion::Ordered);
	assert_eq!(first_operation, legacy_operation);
	assert_ne!(first_operation, later_operation);
	assert_eq!(
		serde_json::to_vec(&first).unwrap(),
		serde_json::to_vec(&later).unwrap()
	);
	assert!(first.get("step").is_none() && first.get("run_revision").is_none());
	assert_eq!(first["task_revision"], json!(3));
	assert_eq!(first["inputs"], legacy_visible["inputs"]);
}

#[test]
fn ordered_dependencies_round_trip_and_reject_unknown_fields() {
	let bank = Bank {
		home: "aidash://home".into(),
		tenant: "acme".into(),
		workspace: uuid::Uuid::now_v7(),
		participant: None,
	};
	let mut memory = Context::status(None);
	memory.banks.push(BankAuthority {
		bank,
		authority: Some("[1,2,{}]".into()),
	});
	let value = dependencies(Some(IndexRevision { revision: None }), Some(&memory)).unwrap();
	let parsed: Dependencies = serde_json::from_value(value.clone()).unwrap();
	assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
	assert_eq!(value["workspace_index"], json!({"revision":null}));
	assert_eq!(value["memory_banks"][0]["authority"], json!("[1,2,{}]"));
	assert_eq!(
		dependencies(None, None).unwrap(),
		json!({"workspace_index":null,"memory_banks":[]})
	);
	let mut tampered = value;
	tampered["step"] = json!(1);
	assert!(serde_json::from_value::<Dependencies>(tampered).is_err());
}
