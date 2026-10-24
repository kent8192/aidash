use super::*;
use rstest::{fixture, rstest};

#[fixture]
fn bundle() -> PolicyBundle {
	serde_json::from_value(json!({
		"tenant": "acme",
		"roles": {"member": {}, "owner": {}},
		"groups": {
			"members": {"roles": ["member"], "assignable": true},
			"administrators": {"roles": ["owner"]}
		},
		"subjects": {
			"root": {"kind": "user", "groups": ["administrators", "members"]},
			"bot": {"kind": "agent", "node": crate::nodes::NodeName::derived("acme")}
		}
	}))
	.unwrap()
}

fn set(values: &[&str]) -> BTreeSet<String> {
	values.iter().map(|value| (*value).to_owned()).collect()
}

#[rstest]
fn approval_cannot_hand_out_an_existing_subject(bundle: PolicyBundle) {
	assert!(matches!(
		approval_target(&bundle, "root", false, &set(&[]), false),
		Err(Error::Conflict(_))
	));
	assert_eq!(
		approval_target(&bundle, "root", true, &set(&[]), false).unwrap(),
		ApprovalTarget::Reapproval
	);
	assert!(matches!(
		approval_target(&bundle, "root", true, &set(&["members"]), false),
		Err(Error::Invalid(_))
	));
}

#[rstest]
fn a_member_who_left_rejoins_with_only_the_invitations_groups(mut bundle: PolicyBundle) {
	assert_eq!(
		approval_target(&bundle, "root", true, &set(&[]), true).unwrap(),
		ApprovalTarget::Rejoin
	);
	assert!(matches!(
		approval_target(&bundle, "root", true, &set(&["administrators"]), true),
		Err(Error::Invalid(_))
	));
	rejoin(&mut bundle, "root", &set(&[])).unwrap();
	assert_eq!(bundle.subjects["root"].groups, set(&["administrators"]));
	rejoin(&mut bundle, "root", &set(&["members"])).unwrap();
	assert_eq!(
		bundle.subjects["root"].groups,
		set(&["administrators", "members"])
	);
}

#[rstest]
#[case(&["administrators"])]
#[case(&["missing"])]
fn new_subjects_receive_only_assignable_groups(bundle: PolicyBundle, #[case] groups: &[&str]) {
	let mut changed = bundle.clone();
	assert!(matches!(
		add_user_subject(&mut changed, "carol", &set(groups)),
		Err(Error::Invalid(_))
	));
	assert!(!changed.subjects.contains_key("carol"));
}

#[rstest]
fn a_new_subject_is_an_undelegated_user_without_roles(mut bundle: PolicyBundle) {
	add_user_subject(&mut bundle, "carol", &set(&["members"])).unwrap();
	let carol = &bundle.subjects["carol"];
	assert_eq!(carol.kind, SubjectKind::User);
	assert_eq!(carol.groups, set(&["members"]));
	assert!(carol.roles.is_empty() && carol.delegated_by.is_none() && carol.enabled);
}

#[rstest]
fn membership_changes_retain_groups_outside_the_assignable_boundary(mut bundle: PolicyBundle) {
	assert!(replace_assignable_memberships(&mut bundle, "root", &set(&[])).unwrap());
	assert_eq!(bundle.subjects["root"].groups, set(&["administrators"]));
	assert!(!replace_assignable_memberships(&mut bundle, "root", &set(&[])).unwrap());
	assert!(matches!(
		replace_assignable_memberships(&mut bundle, "root", &set(&["administrators"])),
		Err(Error::Invalid(_))
	));
	assert!(replace_assignable_memberships(&mut bundle, "bot", &set(&["members"])).unwrap());
	assert!(bundle.subjects["bot"].groups.contains("members"));
}

#[rstest]
fn assignable_flag_is_omitted_from_unmarked_groups(bundle: PolicyBundle) {
	let document = serde_json::to_value(&bundle).unwrap();
	assert_eq!(
		document["groups"]["administrators"],
		json!({"roles":["owner"]})
	);
	assert_eq!(document["groups"]["members"]["assignable"], json!(true));
}
