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
