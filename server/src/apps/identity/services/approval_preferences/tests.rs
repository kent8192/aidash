use super::*;
use aidash_domain::approval_rules::{APPROVAL_ADVANCE, MODE_BYPASS, RuleEffect};
use aidash_domain::identity::tenancy::{MEMBER_TOOL_APPROVAL, MEMBERS, OWNER, initial_policy};
use aidash_domain::policy::{
	Condition, Effect, Operand, Policy, ResourceSelector, Subject, SubjectKind, SubjectSelector,
};
use std::collections::BTreeSet;

fn bundle() -> PolicyBundle {
	let mut bundle = initial_policy("t", true).unwrap();
	bundle.subjects.insert(
		"alice".into(),
		Subject {
			kind: SubjectKind::User,
			groups: [MEMBERS.to_owned()].into(),
			roles: BTreeSet::new(),
			attributes: json!({}),
			enabled: true,
			delegated_by: None,
		},
	);
	bundle
}

fn rule(kind: ApprovalKind, target: RuleTarget) -> ApprovalRule {
	ApprovalRule {
		id: Uuid::new_v4(),
		kind,
		target,
		effect: RuleEffect::Allow,
	}
}

fn allowed(modes: &[ModeChoice], mode: ApprovalMode) -> bool {
	modes
		.iter()
		.find(|choice| choice.mode == mode)
		.unwrap()
		.allowed
}

#[test]
fn a_member_may_use_every_mode_but_bypass_until_the_policy_names_it() {
	// Arrange
	let mut bundle = bundle();

	// Act
	let before = modes(&bundle, "alice", Availability::Usable, true);
	bundle.policies.push(Policy {
		id: "alice-bypass".into(),
		effect: Effect::Allow,
		subjects: SubjectSelector {
			ids: ["alice".to_owned()].into(),
			..Default::default()
		},
		actions: [MODE_BYPASS.to_owned()].into(),
		resources: ResourceSelector {
			kinds: [MODE_RESOURCE.to_owned()].into(),
			ids: ["bypass".to_owned()].into(),
		},
		condition: None,
	});
	let after = modes(&bundle, "alice", Availability::Usable, true);

	// Assert
	assert!(allowed(&before, ApprovalMode::Manual));
	assert!(allowed(&before, ApprovalMode::DontAsk));
	assert!(allowed(&before, ApprovalMode::Auto));
	assert!(!allowed(&before, ApprovalMode::Bypass));
	assert!(allowed(&after, ApprovalMode::Bypass));
}

#[test]
fn an_administrator_still_has_to_be_named_for_bypass() {
	assert!(!allowed(
		&modes(&bundle(), OWNER, Availability::Usable, true),
		ApprovalMode::Bypass
	));
}

fn reason(modes: &[ModeChoice], mode: ApprovalMode) -> Option<ModeReason> {
	modes
		.iter()
		.find(|choice| choice.mode == mode)
		.unwrap()
		.reason
}

#[test]
fn auto_needs_a_decider_that_can_judge_and_says_why_it_is_unavailable() {
	let bundle = bundle();
	for (availability, expected) in [
		(Availability::Usable, (true, None)),
		(
			Availability::Unapproved,
			(false, Some(ModeReason::DeciderUnapproved)),
		),
		(Availability::Missing, (false, Some(ModeReason::NoDecider))),
	] {
		let offered = modes(&bundle, "alice", availability, false);
		assert_eq!(
			(
				allowed(&offered, ApprovalMode::Auto),
				reason(&offered, ApprovalMode::Auto)
			),
			expected
		);
		// What the Decider does not decide stays as the policy leaves it.
		assert!(allowed(&offered, ApprovalMode::Manual));
		assert_eq!(reason(&offered, ApprovalMode::Manual), None);
	}
}

#[test]
fn a_mode_the_policy_withholds_says_policy_whatever_the_node_offers() {
	let offered = modes(&bundle(), "alice", Availability::Missing, false);
	assert_eq!(
		reason(&offered, ApprovalMode::Bypass),
		Some(ModeReason::Policy)
	);
}

#[test]
fn bypass_is_the_persons_to_choose_yet_takes_effect_only_after_they_accept_its_risk() {
	let mut bundle = bundle();
	bundle.policies.push(Policy {
		id: "alice-bypass".into(),
		effect: Effect::Allow,
		subjects: SubjectSelector {
			ids: ["alice".to_owned()].into(),
			..Default::default()
		},
		actions: [MODE_BYPASS.to_owned()].into(),
		resources: ResourceSelector {
			kinds: [MODE_RESOURCE.to_owned()].into(),
			ids: ["bypass".to_owned()].into(),
		},
		condition: None,
	});

	let unaccepted = modes(&bundle, "alice", Availability::Usable, false);
	let accepted = modes(&bundle, "alice", Availability::Usable, true);

	assert!(allowed(&unaccepted, ApprovalMode::Bypass));
	assert_eq!(
		reason(&unaccepted, ApprovalMode::Bypass),
		Some(ModeReason::NotAccepted)
	);
	assert_eq!(reason(&accepted, ApprovalMode::Bypass), None);
}

#[test]
fn a_rule_is_effective_where_the_owner_could_approve_in_advance() {
	// Arrange
	let bundle = bundle();
	let tool = rule(
		ApprovalKind::ToolCall,
		RuleTarget::Exact("tool:crm#invoke".into()),
	);
	let every_tool = rule(ApprovalKind::ToolCall, RuleTarget::Every);
	let origin = rule(
		ApprovalKind::Outbound,
		RuleTarget::Exact("https://example.com".into()),
	);

	// Act + Assert: members approve tool calls themselves, never outbound reads.
	assert!(effective(&bundle, "alice", &tool, &[]));
	assert!(effective(&bundle, "alice", &every_tool, &[]));
	assert!(!effective(&bundle, "alice", &origin, &[]));
	assert!(effective(&bundle, OWNER, &origin, &[]));
}

#[test]
fn an_exact_search_rule_is_effective_on_a_real_destination_the_policy_names() {
	// Arrange: members may approve only the destination `research.example`, and
	// the rule names the provider rather than that destination.
	let mut bundle = bundle();
	bundle.policies.push(Policy {
		id: "alice-research".into(),
		effect: Effect::Allow,
		subjects: SubjectSelector {
			ids: ["alice".to_owned()].into(),
			..Default::default()
		},
		actions: ["capability.approve".to_owned()].into(),
		resources: ResourceSelector {
			kinds: ["research".to_owned()].into(),
			ids: ["research.example".to_owned()].into(),
		},
		// Runtime disclosure resources carry the destination attribute too.
		condition: Some(Condition::Eq {
			left: Operand::Resource {
				path: "/destination".into(),
			},
			right: Operand::Literal {
				value: json!("research.example"),
			},
		}),
	});
	let search = rule(
		ApprovalKind::WebSearch,
		RuleTarget::Exact("research.brave@1".into()),
	);

	// Act + Assert
	assert!(effective(&bundle, "alice", &search, &[]));
}

#[test]
fn a_generation_rule_sees_the_request_attributes_admission_checks() {
	// Arrange: admission passes the generated policy, its root subject and a
	// task id. The settings view has no task yet, but must still carry these
	// attributes so the policy's conditions are evaluated.
	let mut bundle = bundle();
	bundle.policies.push(Policy {
		id: "alice-generation".into(),
		effect: Effect::Allow,
		subjects: SubjectSelector {
			ids: ["alice".to_owned()].into(),
			..Default::default()
		},
		actions: ["generation.approve".to_owned()].into(),
		resources: ResourceSelector {
			kinds: ["generation".to_owned()].into(),
			ids: BTreeSet::new(),
		},
		condition: Some(Condition::All {
			conditions: vec![
				Condition::Eq {
					left: Operand::Resource {
						path: "/policy_id".into(),
					},
					right: Operand::Literal {
						value: json!("gen"),
					},
				},
				Condition::Eq {
					left: Operand::Resource {
						path: "/root_subject".into(),
					},
					right: Operand::Literal {
						value: json!("alice"),
					},
				},
				Condition::Exists {
					value: Operand::Resource {
						path: "/task_id".into(),
					},
				},
			],
		}),
	});
	bundle.policies.push(Policy {
		id: "alice-catalog".into(),
		effect: Effect::Allow,
		subjects: SubjectSelector {
			ids: ["alice".to_owned()].into(),
			..Default::default()
		},
		actions: ["catalog.approve".to_owned()].into(),
		resources: ResourceSelector {
			kinds: ["registry".to_owned()].into(),
			ids: BTreeSet::new(),
		},
		condition: None,
	});
	let template: aidash_domain::registry::Entry = serde_json::from_value(json!({
		"id": "generated",
		"version": "1.0.0",
		"kind": "agent",
		"name": {},
		"description": {}
	}))
	.unwrap();
	let generation = rule(ApprovalKind::Generation, RuleTarget::Exact("gen".into()));

	// Act + Assert
	assert!(effective(
		&bundle,
		"alice",
		&generation,
		&[("gen".to_owned(), template)]
	));
}

#[test]
fn a_rule_for_every_target_applies_to_the_real_ids_the_policy_names() {
	// Arrange: members may approve only the real tool `crm`, not the
	// representative id alone.
	let mut bundle = bundle();
	let approval = bundle
		.policies
		.iter_mut()
		.find(|policy| policy.id == MEMBER_TOOL_APPROVAL)
		.expect("members approve their tool calls");
	approval.resources.ids = ["crm".to_owned()].into();
	approval.condition = Some(Condition::Eq {
		left: Operand::Resource {
			path: "/operation".into(),
		},
		right: Operand::Literal {
			value: json!("invoke"),
		},
	});
	let every = rule(ApprovalKind::ToolCall, RuleTarget::Every);

	// Act + Assert
	assert!(effective(&bundle, "alice", &every, &[]));
}

#[test]
fn a_rule_loses_its_effect_when_the_ceiling_is_lowered() {
	// Arrange
	let mut bundle = bundle();
	let tool = rule(
		ApprovalKind::ToolCall,
		RuleTarget::Exact("tool:crm#invoke".into()),
	);
	assert!(effective(&bundle, "alice", &tool, &[]));

	// Act: a deny of advancing approvals, as a Tenant Administrator would write.
	bundle.policies.push(Policy {
		id: "no-advance".into(),
		effect: Effect::Deny,
		subjects: SubjectSelector {
			any: true,
			..Default::default()
		},
		actions: [APPROVAL_ADVANCE.to_owned()].into(),
		resources: ResourceSelector {
			kinds: ["*".to_owned()].into(),
			ids: BTreeSet::new(),
		},
		condition: None,
	});

	// Assert
	assert!(!effective(&bundle, "alice", &tool, &[]));
}

#[test]
fn a_deny_or_ask_rule_holds_whatever_the_ceiling() {
	// Arrange: members cannot approve outbound reads, nor anything for "mallory".
	let bundle = bundle();
	let origin = |effect| ApprovalRule {
		effect,
		..rule(
			ApprovalKind::Outbound,
			RuleTarget::Exact("https://example.com".into()),
		)
	};

	// Act + Assert
	assert!(!effective(
		&bundle,
		"alice",
		&origin(RuleEffect::Allow),
		&[]
	));
	assert!(effective(&bundle, "alice", &origin(RuleEffect::Deny), &[]));
	assert!(effective(&bundle, "alice", &origin(RuleEffect::Ask), &[]));
	assert!(effective(
		&bundle,
		"mallory",
		&origin(RuleEffect::Deny),
		&[]
	));
}

#[test]
fn a_subject_the_policy_does_not_list_has_no_effective_rule() {
	let tool = rule(ApprovalKind::ToolCall, RuleTarget::Every);
	assert!(!effective(&bundle(), "mallory", &tool, &[]));
}
