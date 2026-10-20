use super::*;
use crate::approval_rules::{MODE_BYPASS, MODE_RESOURCE};
use crate::identity::tenant_administration::{ACTIONS, ADMINISTRATION_ACTIONS};
use rstest::rstest;
use std::collections::BTreeSet;

fn entropy(seed: u128) -> [u8; 16] {
	seed.to_le_bytes()
}

fn allows(bundle: &PolicyBundle, subject: &str, action: &str) -> bool {
	bundle
		.evaluate(&Evaluation {
			subject: subject.into(),
			action: action.into(),
			resource: Resource {
				tenant: bundle.tenant.clone(),
				kind: administration::resource_kind(action).into(),
				id: COLLECTION.into(),
				attributes: json!({}),
			},
			environment: json!({}),
		})
		.allowed
}

#[rstest]
#[case::zero(0)]
#[case::ones(u128::MAX)]
#[case::mixed(0x0123_4567_89ab_cdef_fedc_ba98_7654_3210)]
fn names_are_adjective_noun_and_eight_characters(#[case] seed: u128) {
	let name = TenantName::from_entropy(entropy(seed));
	let parts: Vec<_> = name.as_str().split('-').collect();
	assert_eq!(parts.len(), 3, "{name}");
	assert!(words::ADJECTIVES.contains(&parts[0]));
	assert!(words::NOUNS.contains(&parts[1]));
	assert_eq!(parts[2].len(), 8);
	assert!(
		parts[2]
			.bytes()
			.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
	);
	assert!(name.as_str().len() <= 63);
	identifier(name.as_str()).unwrap();
}

#[test]
fn names_rarely_collide() {
	let names: BTreeSet<_> = (0..10_000u128)
		.map(|seed| TenantName::from_entropy(entropy(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15))))
		.collect();
	assert_eq!(names.len(), 10_000);
}

#[test]
fn word_lists_are_unique_short_and_lowercase() {
	for list in [&words::ADJECTIVES[..], &words::NOUNS[..]] {
		let unique: BTreeSet<_> = list.iter().collect();
		assert_eq!(unique.len(), list.len());
		assert!(
			list.iter()
				.all(|word| word.len() <= 9 && word.bytes().all(|b| b.is_ascii_lowercase()))
		);
	}
}

#[rstest]
#[case::trimmed("  Research team ", Some("Research team"))]
#[case::unicode("研究チーム", Some("研究チーム"))]
#[case::sixty_four(&"a".repeat(64), Some(&*"a".repeat(64)))]
#[case::empty("   ", None)]
#[case::too_long(&"a".repeat(65), None)]
#[case::control("team\u{7}", None)]
#[case::newline("team\nname", None)]
fn labels_are_trimmed_and_bounded(#[case] input: &str, #[case] expected: Option<&str>) {
	assert_eq!(label(input).ok().as_deref(), expected);
}

fn at(seconds: i64) -> DateTime<Utc> {
	DateTime::from_timestamp(1_800_000_000 + seconds, 0).unwrap()
}

fn open() -> TenancyRules {
	TenancyRules {
		tenant_creation: true,
		..TenancyRules::of(Offering::SelfHosted)
	}
}

/// #238's table: Dedicated has the Self-hosted column, Cloud lets people start
/// Tenants, and every Offering has Invitations and the same limits.
#[rstest]
#[case(Offering::SelfHosted, false)]
#[case(Offering::Cloud, true)]
#[case(Offering::Dedicated, false)]
fn each_offering_supplies_its_column_of_defaults(
	#[case] offering: Offering,
	#[case] self_service: bool,
) {
	assert_eq!(
		TenancyRules::of(offering),
		TenancyRules {
			personal_tenants: self_service,
			tenant_creation: self_service,
			creation_per_hour: 5,
			creation_total: 20,
			invitations: true,
			deletion_grace_days: 30,
		}
	);
}

#[test]
fn creation_needs_the_rule() {
	assert_eq!(
		check_creation(&TenancyRules::of(Offering::SelfHosted), at(0), &[], 0),
		Err(CreationRefusal::NotAllowed)
	);
	assert_eq!(check_creation(&open(), at(0), &[], 0), Ok(()));
}

#[test]
fn the_hourly_limit_waits_for_the_oldest_creation_in_the_window() {
	let now = at(3_600);
	let recent = [at(600), at(100), at(1_200), at(2_000), at(3_000), at(-10)];
	assert_eq!(
		check_creation(&open(), now, &recent, 6),
		Err(CreationRefusal::HourlyLimit {
			retry_after_seconds: 100
		})
	);
	assert_eq!(check_creation(&open(), now, &recent[..4], 4), Ok(()));
}

#[test]
fn the_total_limit_counts_every_tenant_not_purged() {
	assert_eq!(
		check_creation(&open(), at(0), &[], 20),
		Err(CreationRefusal::TotalLimit)
	);
	assert_eq!(check_creation(&open(), at(0), &[], 19), Ok(()));
}

#[test]
fn a_zero_hourly_limit_allows_nothing() {
	let rules = TenancyRules {
		creation_per_hour: 0,
		..open()
	};
	assert_eq!(
		check_creation(&rules, at(0), &[], 0),
		Err(CreationRefusal::NotAllowed)
	);
}

#[test]
fn the_creator_administers_and_members_use() {
	let mut bundle = initial_policy("brave-otter-k3m9q2xa", true).unwrap();
	for action in ACTIONS
		.iter()
		.chain([TENANT_UPDATE, TENANT_DELETE, "catalog.approve"].iter())
	{
		assert!(allows(&bundle, OWNER, action), "{action}");
	}
	assert!(bundle.groups[ADMINISTRATORS].assignable);
	assert!(bundle.groups[MEMBERS].assignable);
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
	for action in [
		"workspace.create",
		"run.read",
		"registry.create",
		"marketplace.install",
		"installation.create",
		"tool.invoke",
		"credential.create",
		"semantic.index.manage",
	] {
		assert!(allows(&bundle, "alice", action), "{action}");
	}
	for action in ACTIONS.iter().chain(
		[
			TENANT_UPDATE,
			TENANT_DELETE,
			"catalog.approve",
			"provider_credential.create",
			"research.disclose",
			"retention.update",
		]
		.iter(),
	) {
		assert!(!allows(&bundle, "alice", action), "{action}");
	}
}

#[test]
fn members_approve_tool_calls_and_nothing_else_they_could_be_asked_for() {
	let bundle = with_member(initial_policy("t", true).unwrap(), "alice", MEMBERS);
	let evaluate = |subject: &str, kind: &str, action: &str| {
		bundle
			.evaluate(&Evaluation {
				subject: subject.into(),
				action: action.into(),
				resource: Resource {
					tenant: bundle.tenant.clone(),
					kind: kind.into(),
					id: "tool-1".into(),
					attributes: json!({}),
				},
				environment: json!({}),
			})
			.allowed
	};
	assert!(evaluate("alice", "tool", "capability.approve"));
	for kind in ["outbound", "research", "generation"] {
		assert!(!evaluate("alice", kind, "capability.approve"), "{kind}");
	}
	// Calling without approval is never granted by default, not even by an
	// administrator's wildcard: the Tenant has to name it.
	assert!(!evaluate("alice", "tool", "tool.call_unapproved"));
	assert!(!evaluate(OWNER, "tool", "tool.call_unapproved"));
}

#[test]
fn members_may_approve_in_advance_and_use_auto_but_bypass_has_to_be_named() {
	let bundle = with_member(initial_policy("t", true).unwrap(), "alice", MEMBERS);
	let evaluate = |subject: &str, kind: &str, id: &str, action: &str| {
		bundle
			.evaluate(&Evaluation {
				subject: subject.into(),
				action: action.into(),
				resource: Resource {
					tenant: bundle.tenant.clone(),
					kind: kind.into(),
					id: id.into(),
					attributes: json!({}),
				},
				environment: json!({}),
			})
			.allowed
	};
	// Advancing is allowed on every kind, yet only what the member could
	// approve themselves is within the ceiling.
	for kind in ["tool", "outbound", "research", "generation"] {
		assert!(evaluate("alice", kind, "x", APPROVAL_ADVANCE), "{kind}");
	}
	assert!(!evaluate("alice", "outbound", "x", "capability.approve"));
	assert!(evaluate("alice", MODE_RESOURCE, "auto", MODE_AUTO));
	for subject in ["alice", OWNER] {
		assert!(
			!evaluate(subject, MODE_RESOURCE, "bypass", MODE_BYPASS),
			"{subject}"
		);
	}
}

#[test]
fn only_administrators_read_a_members_approval_settings() {
	// Arrange
	let bundle = with_member(initial_policy("t", true).unwrap(), "alice", MEMBERS);
	let read = administration::APPROVAL_READ;

	// Act + Assert: the action is judged on the member, and members never hold it.
	assert_eq!(administration::resource_kind(read), "subject");
	assert!(allows(&bundle, OWNER, read));
	assert!(!allows(&bundle, "alice", read));
	assert!(ACTIONS.contains(&read));
	assert!(!ADMINISTRATION_ACTIONS.contains(&read));
}

#[test]
fn the_mode_actions_are_judged_on_the_approval_mode_resource() {
	for action in [crate::approval_rules::MODE_AUTO, MODE_BYPASS] {
		assert_eq!(administration::resource_kind(action), MODE_RESOURCE);
	}
}

#[test]
fn a_wildcard_allow_never_grants_an_explicit_action_but_a_wildcard_deny_covers_it() {
	let mut bundle = initial_policy("t", true).unwrap();
	for action in crate::policy::EXPLICIT_ACTIONS {
		assert!(!allows(&bundle, OWNER, action), "{action}");
	}
	let mut named = bundle.policies[0].clone();
	named.id = "named".into();
	named.actions = crate::policy::EXPLICIT_ACTIONS
		.iter()
		.map(|action| (*action).to_owned())
		.collect();
	bundle.policies.push(named);
	for action in crate::policy::EXPLICIT_ACTIONS {
		assert!(allows(&bundle, OWNER, action), "{action}");
	}
	let mut deny = bundle.policies[0].clone();
	deny.id = "deny-all".into();
	deny.effect = crate::policy::Effect::Deny;
	deny.actions = ["*".to_owned()].into();
	bundle.policies.push(deny);
	for action in crate::policy::EXPLICIT_ACTIONS {
		assert!(!allows(&bundle, OWNER, action), "{action}");
	}
}

#[test]
fn a_tenant_without_a_creator_starts_empty() {
	let bundle = initial_policy("calm-heron-00000000", false).unwrap();
	assert!(bundle.subjects.is_empty());
	assert_eq!(bundle.policies.len(), 3);
}

fn with_member(mut bundle: PolicyBundle, name: &str, group: &str) -> PolicyBundle {
	bundle.subjects.insert(
		name.into(),
		Subject {
			kind: SubjectKind::User,
			groups: [group.to_owned()].into(),
			roles: BTreeSet::new(),
			attributes: json!({}),
			enabled: true,
			delegated_by: None,
		},
	);
	bundle
}

#[test]
fn an_administrator_counts_and_a_member_does_not() {
	let bundle = with_member(initial_policy("t", true).unwrap(), "alice", MEMBERS);
	assert!(has_unconditional_administrator(&bundle, [OWNER]));
	assert!(!has_unconditional_administrator(&bundle, ["alice"]));
	assert!(!has_unconditional_administrator(&bundle, ["missing"]));
	assert!(!has_unconditional_administrator(&bundle, []));
}

#[test]
fn an_administrator_who_cannot_delete_the_tenant_does_not_count() {
	let mut bundle = with_member(initial_policy("t", true).unwrap(), "alice", MEMBERS);
	bundle.policies.push(Policy {
		id: "alice-administers".into(),
		effect: Effect::Allow,
		subjects: SubjectSelector {
			ids: ["alice".into()].into(),
			..Default::default()
		},
		actions: ADMINISTRATION_ACTIONS
			.iter()
			.filter(|action| **action != TENANT_DELETE)
			.map(|action| (*action).to_owned())
			.collect(),
		resources: ResourceSelector {
			kinds: ["*".into()].into(),
			ids: BTreeSet::new(),
		},
		condition: None,
	});
	bundle.validate().unwrap();
	assert!(!has_unconditional_administrator(&bundle, ["alice"]));
}

#[test]
fn conditional_or_narrowed_administration_does_not_count() {
	let mut bundle = with_member(initial_policy("t", false).unwrap(), "bob", MEMBERS);
	let mut conditional = bundle.policies[0].clone();
	conditional.id = "conditional".into();
	conditional.subjects = SubjectSelector {
		ids: ["bob".into()].into(),
		..Default::default()
	};
	conditional.condition = Some(
		serde_json::from_value(json!({
		"op":"eq","left":{"source":"environment","path":"/transport"},
		"right":{"source":"literal","value":"api"}}))
		.unwrap(),
	);
	bundle.policies.push(conditional.clone());
	bundle.validate().unwrap();
	assert!(!has_unconditional_administrator(&bundle, ["bob"]));
	let mut narrowed = conditional;
	narrowed.id = "narrowed".into();
	narrowed.condition = None;
	narrowed.resources.ids = [COLLECTION.to_owned()].into();
	bundle.policies.push(narrowed);
	bundle.validate().unwrap();
	assert!(!has_unconditional_administrator(&bundle, ["bob"]));
}

#[test]
fn any_deny_on_an_administration_action_counts_against() {
	let mut bundle = initial_policy("t", true).unwrap();
	bundle.policies.push(Policy {
		id: "no-invites-on-weekends".into(),
		effect: Effect::Deny,
		subjects: SubjectSelector {
			ids: [OWNER.into()].into(),
			..Default::default()
		},
		actions: [ADMINISTRATION_ACTIONS[6].to_owned()].into(),
		resources: ResourceSelector {
			kinds: ["invitation".into()].into(),
			ids: ["someone-else".into()].into(),
		},
		condition: Some(
			serde_json::from_value(json!({
				"op":"eq","left":{"source":"environment","path":"/day"},
				"right":{"source":"literal","value":"sunday"}}))
			.unwrap(),
		),
	});
	bundle.validate().unwrap();
	assert!(!has_unconditional_administrator(&bundle, [OWNER]));
}

#[test]
fn an_administrator_who_cannot_repair_the_policy_does_not_count() {
	let mut bundle = initial_policy("t", true).unwrap();
	assert!(ADMINISTRATION_ACTIONS.contains(&"policy.update"));
	bundle.policies.push(Policy {
		id: "no-policy-writes".into(),
		effect: Effect::Deny,
		subjects: SubjectSelector {
			ids: [OWNER.into()].into(),
			..Default::default()
		},
		actions: ["policy.update".to_owned()].into(),
		resources: ResourceSelector {
			kinds: ["policy".into()].into(),
			ids: BTreeSet::new(),
		},
		condition: None,
	});
	bundle.validate().unwrap();
	assert!(!has_unconditional_administrator(&bundle, [OWNER]));
}

#[rstest]
#[case::default(None, Some(7 * 24 * 3600))]
#[case::shorter(Some(3600), Some(3600))]
#[case::too_short(Some(59), None)]
#[case::too_long(Some(7 * 24 * 3600 + 1), None)]
fn invitations_last_at_most_seven_days(
	#[case] requested: Option<i64>,
	#[case] expected: Option<i64>,
) {
	assert_eq!(
		invitation::lifetime(requested)
			.ok()
			.map(|duration| duration.num_seconds()),
		expected
	);
}
