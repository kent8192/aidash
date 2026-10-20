use super::*;
use ApprovalKind::{Generation, Outbound, PageReading, ToolCall, WebSearch};
use ApprovalMode::{Auto, Bypass, DontAsk, Manual};
use Resolution::{Approve, Ask, Judge, Proceed, Refuse};
use rstest::rstest;

const TOOL: &str = "tool:crm#invoke";
const OPEN: Ceiling = Ceiling {
	may_approve: true,
	mode_allowed: true,
};
const NO_APPROVAL: Ceiling = Ceiling {
	may_approve: false,
	mode_allowed: true,
};
const NO_MODE: Ceiling = Ceiling {
	may_approve: true,
	mode_allowed: false,
};

fn id(n: u128) -> Uuid {
	Uuid::from_u128(n)
}

fn rule(n: u128, effect: RuleEffect) -> ApprovalRule {
	ApprovalRule {
		id: id(n),
		kind: ToolCall,
		target: RuleTarget::Exact(TOOL.into()),
		effect,
	}
}

fn settings(mode: ApprovalMode, rules: Vec<ApprovalRule>) -> Settings {
	Settings {
		mode,
		rules,
		bypass_accepted: true,
	}
}

fn decide(settings: &Settings, ceiling: Ceiling) -> Resolution {
	resolve(
		TenantStance::NeedsApproval,
		Some(settings),
		ceiling,
		ToolCall,
		TOOL,
		false,
	)
}

#[rstest]
#[case::forbidden_beats_every_allowance(TenantStance::Forbidden, Refuse(RefusedBy::Tenant))]
#[case::direct_permission_needs_nothing(TenantStance::Direct, Proceed)]
fn the_tenant_policy_decides_before_any_rule_or_mode(
	#[case] stance: TenantStance,
	#[case] expected: Resolution,
) {
	let bypass = settings(Bypass, vec![rule(1, RuleEffect::Deny)]);

	assert_eq!(
		resolve(stance, Some(&bypass), OPEN, ToolCall, TOOL, false),
		expected
	);
	assert_eq!(
		resolve(stance, None, NO_APPROVAL, ToolCall, TOOL, false),
		expected
	);
}

#[rstest]
#[case::within_the_ceiling(OPEN)]
#[case::beyond_the_ceiling(NO_APPROVAL)]
fn without_a_person_of_the_tenant_a_designated_approver_is_asked(#[case] ceiling: Ceiling) {
	assert_eq!(
		resolve(
			TenantStance::NeedsApproval,
			None,
			ceiling,
			ToolCall,
			TOOL,
			false
		),
		Ask
	);
}

#[rstest]
#[case::manual(Manual)]
#[case::auto(Auto)]
#[case::dont_ask(DontAsk)]
#[case::bypass(Bypass)]
fn a_deny_rule_refuses_in_every_mode_beyond_or_within_the_ceiling(#[case] mode: ApprovalMode) {
	let denying = settings(mode, vec![rule(7, RuleEffect::Deny)]);

	for ceiling in [OPEN, NO_APPROVAL, NO_MODE] {
		assert_eq!(decide(&denying, ceiling), Refuse(RefusedBy::Rule(id(7))));
	}
}

#[rstest]
#[case::deny_beats_ask_and_allow(
	vec![rule(1, RuleEffect::Allow), rule(2, RuleEffect::Ask), rule(3, RuleEffect::Deny)],
	Refuse(RefusedBy::Rule(id(3)))
)]
#[case::ask_beats_allow(vec![rule(1, RuleEffect::Allow), rule(2, RuleEffect::Ask)], Ask)]
#[case::allow_alone_approves(vec![rule(1, RuleEffect::Allow)], Approve(GivenBy::Rule(id(1))))]
#[case::the_smallest_id_of_the_winning_effect_decides(
	vec![rule(9, RuleEffect::Deny), rule(4, RuleEffect::Deny), rule(2, RuleEffect::Allow)],
	Refuse(RefusedBy::Rule(id(4)))
)]
#[case::the_smallest_allow_names_the_approval(
	vec![rule(6, RuleEffect::Allow), rule(5, RuleEffect::Allow)],
	Approve(GivenBy::Rule(id(5)))
)]
fn rules_apply_deny_before_ask_before_allow(
	#[case] rules: Vec<ApprovalRule>,
	#[case] expected: Resolution,
) {
	assert_eq!(decide(&settings(Manual, rules), OPEN), expected);
}

#[rstest]
#[case::manual(Manual, Ask)]
#[case::auto(Auto, Ask)]
#[case::bypass(Bypass, Ask)]
#[case::dont_ask(DontAsk, Refuse(RefusedBy::DontAsk))]
fn an_explicit_ask_asks_in_every_mode_but_dont_ask(
	#[case] mode: ApprovalMode,
	#[case] expected: Resolution,
) {
	let asking = settings(mode, vec![rule(1, RuleEffect::Ask)]);

	assert_eq!(decide(&asking, OPEN), expected);
}

#[rstest]
#[case::manual(Manual, Approve(GivenBy::Rule(id(1))))]
#[case::auto(Auto, Approve(GivenBy::Rule(id(1))))]
#[case::dont_ask(DontAsk, Approve(GivenBy::Rule(id(1))))]
#[case::bypass(Bypass, Approve(GivenBy::Rule(id(1))))]
fn an_allow_within_the_ceiling_approves_in_every_mode(
	#[case] mode: ApprovalMode,
	#[case] expected: Resolution,
) {
	let allowing = settings(mode, vec![rule(1, RuleEffect::Allow)]);

	assert_eq!(decide(&allowing, OPEN), expected);
}

#[rstest]
#[case::manual_asks(Manual, Ask)]
#[case::auto_asks(Auto, Ask)]
#[case::bypass_asks(Bypass, Ask)]
#[case::dont_ask_refuses(DontAsk, Refuse(RefusedBy::DontAsk))]
fn an_allow_beyond_the_ceiling_has_no_effect(
	#[case] mode: ApprovalMode,
	#[case] expected: Resolution,
) {
	let allowing = settings(mode, vec![rule(1, RuleEffect::Allow)]);

	assert_eq!(decide(&allowing, NO_APPROVAL), expected);
}

#[rstest]
#[case::manual_within(Manual, OPEN, false, Ask)]
#[case::manual_beyond(Manual, NO_APPROVAL, false, Ask)]
#[case::dont_ask_within(DontAsk, OPEN, false, Refuse(RefusedBy::DontAsk))]
#[case::dont_ask_beyond(DontAsk, NO_APPROVAL, false, Refuse(RefusedBy::DontAsk))]
#[case::bypass_within(Bypass, OPEN, false, Approve(GivenBy::Bypass))]
#[case::bypass_beyond_the_ceiling(Bypass, NO_APPROVAL, false, Ask)]
#[case::bypass_not_allowed_acts_as_manual(Bypass, NO_MODE, false, Ask)]
#[case::auto_within(Auto, OPEN, false, Judge)]
#[case::auto_beyond_the_ceiling(Auto, NO_APPROVAL, false, Ask)]
#[case::auto_not_allowed_acts_as_manual(Auto, NO_MODE, false, Ask)]
#[case::auto_suspended_asks(Auto, OPEN, true, Ask)]
#[case::suspension_changes_nothing_else(Bypass, OPEN, true, Approve(GivenBy::Bypass))]
fn the_mode_decides_what_no_rule_covers(
	#[case] mode: ApprovalMode,
	#[case] ceiling: Ceiling,
	#[case] auto_suspended: bool,
	#[case] expected: Resolution,
) {
	let resolution = resolve(
		TenantStance::NeedsApproval,
		Some(&settings(mode, vec![])),
		ceiling,
		ToolCall,
		TOOL,
		auto_suspended,
	);

	assert_eq!(resolution, expected);
}

#[test]
fn a_mode_the_policy_does_not_allow_acts_as_manual_for_an_ask_rule() {
	let asking = settings(DontAsk, vec![rule(1, RuleEffect::Ask)]);

	assert_eq!(decide(&asking, NO_MODE), Ask);
}

#[rstest]
#[case::exact_target(
	ToolCall,
	RuleTarget::Exact(TOOL.into()),
	ToolCall,
	TOOL,
	Approve(GivenBy::Rule(id(1)))
)]
#[case::every_target_of_the_kind(
	ToolCall,
	RuleTarget::Every,
	ToolCall,
	"tool:other#invoke",
	Approve(GivenBy::Rule(id(1)))
)]
#[case::another_target(ToolCall, RuleTarget::Exact(TOOL.into()), ToolCall, "tool:other#invoke", Ask)]
#[case::another_operation(ToolCall, RuleTarget::Exact(TOOL.into()), ToolCall, "tool:crm#other", Ask)]
#[case::every_covers_no_other_kind(ToolCall, RuleTarget::Every, Outbound, TOOL, Ask)]
#[case::exact_target_of_another_kind(
	Outbound,
	RuleTarget::Exact("https://example.com".into()),
	WebSearch,
	"https://example.com",
	Ask
)]
#[case::web_search_provider(
	WebSearch,
	RuleTarget::Exact("research.brave@1".into()),
	WebSearch,
	"research.brave@1",
	Approve(GivenBy::Rule(id(1)))
)]
#[case::page_host(
	PageReading,
	RuleTarget::Exact("example.com".into()),
	PageReading,
	"example.com",
	Approve(GivenBy::Rule(id(1)))
)]
#[case::generation_policy(
	Generation,
	RuleTarget::Every,
	Generation,
	"policy",
	Approve(GivenBy::Rule(id(1)))
)]
fn a_rule_covers_only_its_kind_and_its_target(
	#[case] rule_kind: ApprovalKind,
	#[case] target: RuleTarget,
	#[case] kind: ApprovalKind,
	#[case] name: &str,
	#[case] expected: Resolution,
) {
	let allowing = settings(
		Manual,
		vec![ApprovalRule {
			id: id(1),
			kind: rule_kind,
			target,
			effect: RuleEffect::Allow,
		}],
	);

	assert_eq!(
		resolve(
			TenantStance::NeedsApproval,
			Some(&allowing),
			OPEN,
			kind,
			name,
			false
		),
		expected
	);
}

#[rstest]
#[case::manual_needs_no_policy(Manual, true, true, true)]
#[case::dont_ask_needs_no_policy(DontAsk, true, true, true)]
#[case::auto_needs_the_policy(Auto, true, true, true)]
#[case::auto_without_the_policy(Auto, true, false, false)]
#[case::bypass_needs_the_policy_and_acceptance(Bypass, true, true, true)]
#[case::bypass_without_the_policy(Bypass, true, false, false)]
#[case::bypass_without_acceptance(Bypass, false, true, false)]
fn the_ceiling_follows_the_policy_and_the_recorded_acceptance(
	#[case] mode: ApprovalMode,
	#[case] accepted: bool,
	#[case] mode_policy: bool,
	#[case] allowed: bool,
) {
	let person = Settings {
		mode,
		rules: vec![],
		bypass_accepted: accepted,
	};

	let ceiling = person.ceiling(Standing {
		approver: true,
		advance: true,
		mode: mode_policy,
	});

	assert_eq!(ceiling.mode_allowed, allowed);
	assert!(ceiling.may_approve);
}

#[rstest]
#[case::both(true, true, true)]
#[case::not_an_approver(false, true, false)]
#[case::no_advance(true, false, false)]
fn a_person_approves_in_advance_only_what_they_could_approve_and_the_policy_lets_them(
	#[case] approver: bool,
	#[case] advance: bool,
	#[case] may_approve: bool,
) {
	let ceiling = Settings::default().ceiling(Standing {
		approver,
		advance,
		mode: true,
	});

	assert_eq!(ceiling.may_approve, may_approve);
}

#[test]
fn only_auto_and_bypass_need_the_policy_to_name_them() {
	assert_eq!(Manual.policy_requirement(), None);
	assert_eq!(DontAsk.policy_requirement(), None);
	assert_eq!(Auto.policy_requirement(), Some((MODE_AUTO, "auto")));
	assert_eq!(Bypass.policy_requirement(), Some((MODE_BYPASS, "bypass")));
}

#[test]
fn rules_and_modes_use_the_documented_wire_forms() {
	assert_eq!(
		serde_json::to_value(RuleTarget::Every).unwrap(),
		json!("every")
	);
	assert_eq!(
		serde_json::to_value(RuleTarget::Exact("example.com".into())).unwrap(),
		json!({"exact":"example.com"})
	);
	assert_eq!(
		serde_json::from_value::<RuleTarget>(json!({"exact":"*"})).unwrap(),
		RuleTarget::Exact("*".into())
	);
	assert_eq!(
		serde_json::to_value([Manual, Auto, DontAsk, Bypass]).unwrap(),
		json!(["manual", "auto", "dont_ask", "bypass"])
	);
	assert_eq!(
		serde_json::to_value([ToolCall, Outbound, WebSearch, PageReading, Generation]).unwrap(),
		json!([
			"tool_call",
			"outbound",
			"web_search",
			"page_reading",
			"generation"
		])
	);
	assert_eq!(ApprovalMode::default(), Manual);
	let stored = rule(3, RuleEffect::Deny);
	let round = serde_json::to_value(&stored).unwrap();
	assert_eq!(
		round,
		json!({"id":id(3),"kind":"tool_call","target":{"exact":TOOL},"effect":"deny"})
	);
	assert_eq!(
		serde_json::from_value::<ApprovalRule>(round).unwrap(),
		stored
	);
}

#[test]
fn records_name_what_gave_or_refused_an_approval() {
	assert_eq!(GivenBy::Rule(id(1)).data(), json!({"rule":id(1)}));
	assert_eq!(GivenBy::Bypass.data(), json!({"mode":"bypass"}));
	assert_eq!(RefusedBy::Rule(id(2)).data(), json!({"rule":id(2)}));
	assert_eq!(RefusedBy::DontAsk.data(), json!({"mode":"dont_ask"}));
	assert_eq!(RefusedBy::Tenant.data(), json!({"policy":"tenant"}));
	assert_eq!(
		GivenBy::Rule(id(1)).to_string(),
		format!("Approval Rule {}", id(1))
	);
	assert_eq!(GivenBy::Bypass.to_string(), "the Bypass Approval Mode");
}
