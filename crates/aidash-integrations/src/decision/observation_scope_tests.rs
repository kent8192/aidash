use super::*;

fn observer(hook: Hook) -> DeciderConfig {
	let contract = hook.contract();
	DeciderConfig {
		hook,
		answer_type: AnswerType::Noul,
		description: "Independent observation".into(),
		provider_credential: "openrouter".into(),
		routes: vec![DeciderRoute::jev("typesafe/jev-1.13")],
		builder: contract.builder.into(),
		option_source: contract.option_source.into(),
		rule: contract.rule.into(),
		mode: Mode::Shadow,
		max_calls_per_run: 10,
		state_retention: Default::default(),
	}
}
#[test]
fn an_observer_uses_its_real_scope_without_inventing_a_generation_run() {
	let observation = Uuid::new_v4();
	for (label, hook, run, owner, maintenance, accepted) in [
		(
			"actual Generation observation",
			Hook::ApprovalObservation,
			None,
			Some(observation),
			None,
			true,
		),
		(
			"missing scope",
			Hook::ApprovalObservation,
			None,
			None,
			None,
			false,
		),
		(
			"mixed Run and observer",
			Hook::ApprovalObservation,
			Some(Uuid::new_v4()),
			Some(observation),
			None,
			false,
		),
		(
			"nil observer",
			Hook::ApprovalObservation,
			None,
			Some(Uuid::nil()),
			None,
			false,
		),
		(
			"maintenance is independent",
			Hook::ApprovalObservation,
			None,
			Some(observation),
			Some(aidash_application::provider_access::MaintenancePurpose::MemoryIndexing),
			false,
		),
		(
			"compaction cannot impersonate observation",
			Hook::Compaction,
			None,
			Some(observation),
			None,
			false,
		),
	] {
		let context = Context {
			tenant: "tenant".into(),
			run,
			observation: owner,
			maintenance,
			..Default::default()
		};
		let provider = RouteDecisionProvider::new(
			observer(hook),
			"native-observer-adapter".into(),
			Arc::new(aidash_application::provider_access::KeylessAccess::default()),
			context,
			Arc::new(ByteBudget),
			Duration::from_secs(1),
		);
		assert_eq!(provider.is_ok(), accepted, "{label}");
	}
}
