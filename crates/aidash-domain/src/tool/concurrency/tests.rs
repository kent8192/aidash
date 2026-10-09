use super::*;
use crate::tool::builtin_contract;
use rstest::rstest;
use serde_json::json;

const CEILINGS: ReadCeilings = ReadCeilings {
	read_bytes: 16384,
	search_bytes: 32768,
};

#[rstest]
#[case(ResourceAccess::Shared, ResourceAccess::Shared, false)]
#[case(ResourceAccess::Shared, ResourceAccess::Exclusive, true)]
#[case(ResourceAccess::Exclusive, ResourceAccess::Shared, true)]
#[case(ResourceAccess::Exclusive, ResourceAccess::Exclusive, true)]
fn only_exclusive_access_to_the_same_resource_conflicts(
	#[case] first: ResourceAccess,
	#[case] second: ResourceAccess,
	#[case] conflict: bool,
) {
	let claim = |resource, access| ResourceClaim { resource, access };
	assert_eq!(
		claim(Resource::PinnedSkills, first).conflicts(&claim(Resource::PinnedSkills, second)),
		conflict
	);
	assert!(!claim(Resource::PinnedSkills, first).conflicts(&claim(Resource::WorkingArea, second)));
}

#[test]
fn only_provider_verified_reads_declare_shared_read() {
	let shared = [
		"task_create",
		"task_assign",
		"task_delegate",
		"artifact_publish",
		"workspace_message",
		"memory_mutate",
		"memory_recall",
		"memory_reflect",
		"human_request",
		"agent_discover",
		"workspace_read",
		"workspace_observe",
		"workspace_wait",
		"skill_read",
		"skill_list",
		"skill_load",
		"file_read",
		"file_search",
		"shell",
		"shell_poll",
		"shell_cancel",
		"code_interpreter",
		"python_install",
		"python_poll",
		"python_cancel",
		"apply_patch",
		"file_share",
		"outbound_get",
	]
	.into_iter()
	.filter(|name| {
		let behavior = builtin_contract(name).unwrap().behavior;
		assert_eq!(
			batchable(&behavior),
			behavior.concurrency == Concurrency::SharedRead,
			"{name} declares shared reads without a batchable behavior"
		);
		batchable(&behavior)
	})
	.collect::<Vec<_>>();
	assert_eq!(
		shared,
		["skill_list", "skill_load", "file_read", "file_search"]
	);
}

#[test]
fn sequential_contracts_keep_their_pinned_serialization() {
	let mut behavior = builtin_contract("memory_recall").unwrap().behavior;
	assert!(
		serde_json::to_value(&behavior)
			.unwrap()
			.get("concurrency")
			.is_none()
	);
	behavior.concurrency = Concurrency::SharedRead;
	assert_eq!(
		serde_json::to_value(&behavior).unwrap()["concurrency"],
		"shared_read"
	);
}

#[rstest]
#[case(None, Concurrency::SharedRead)]
#[case(Some(Concurrency::SharedRead), Concurrency::SharedRead)]
#[case(Some(Concurrency::Sequential), Concurrency::Sequential)]
fn restrictions_only_lower_concurrency(
	#[case] restriction: Option<Concurrency>,
	#[case] expected: Concurrency,
) {
	assert_eq!(Concurrency::SharedRead.narrowed(restriction), expected);
	assert_eq!(
		Concurrency::Sequential.narrowed(restriction),
		Concurrency::Sequential
	);
}

#[rstest]
#[case(json!({"file_id":"00000000-0000-0000-0000-000000000001","representation":"text"}), Some(16384))]
#[case(json!({"file_id":"00000000-0000-0000-0000-000000000001","representation":"text","max_bytes":64}), Some(64))]
#[case(json!({"file_id":"00000000-0000-0000-0000-000000000001","representation":"text","max_bytes":1_000_000}), Some(16384))]
#[case(json!({"file_id":"00000000-0000-0000-0000-000000000001","representation":"metadata"}), Some(0))]
#[case(json!({"file_id":"00000000-0000-0000-0000-000000000001","representation":"model_input","expected_digest":"d"}), None)]
#[case(json!({"file_id":"not-a-uuid","representation":"text"}), None)]
fn file_reads_are_bounded_by_the_node_ceiling_and_media_stays_sequential(
	#[case] input: Value,
	#[case] bound: Option<usize>,
) {
	let call = core_call("file_read", &input, CEILINGS);
	assert_eq!(call.as_ref().map(|call| call.output_bytes), bound);
	if let Some(call) = call {
		assert_eq!(call.claims, [ResourceClaim::shared(Resource::WorkingArea)]);
	}
}

#[test]
fn skill_loads_exclude_other_skill_record_users_but_not_file_reads() {
	let load = core_call(
		"skill_load",
		&json!({"skill_id":"00000000-0000-0000-0000-000000000001","expected_digest":"d"}),
		CEILINGS,
	)
	.unwrap();
	let list = core_call("skill_list", &json!({"limit":2}), CEILINGS).unwrap();
	let search = core_call(
		"file_search",
		&json!({"query":"x","mode":"literal","scope":"working"}),
		CEILINGS,
	)
	.unwrap();
	assert!(load.conflicts(&load.clone()));
	assert!(load.conflicts(&list));
	assert!(!list.conflicts(&list.clone()));
	assert!(!load.conflicts(&search));
	assert_eq!(list.output_bytes, 2 * SKILL_METADATA_BYTES);
	assert_eq!(search.output_bytes, CEILINGS.search_bytes);
}

#[test]
fn undeclared_operations_have_no_concurrent_derivation() {
	for operation in [
		"skill_read",
		"workspace_read",
		"outbound_get",
		"shell",
		"apply_patch",
	] {
		assert!(core_call(operation, &json!({}), CEILINGS).is_none());
	}
}
