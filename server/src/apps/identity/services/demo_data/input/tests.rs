use super::*;
use rstest::rstest;
use serde_json::json;

const ENTRY: &str = r#"{"id":"helper","version":"1.0.0","kind":"tool","name":{"en":"Helper"},"description":{"en":"Helps"}}"#;

fn structure() -> Value {
	json!({
		"tenants": [{
			"key": "acme",
			"label": "Acme Research",
			"subjects": [{"id": "dana", "groups": ["members"]}],
			"registry": [serde_json::from_str::<Value>(ENTRY).unwrap()],
			"approvals": [{"entry": {"id": "helper", "version": "1.0.0"}, "groups": ["members"]}]
		}]
	})
}

fn people() -> Value {
	json!({"people": [{
		"email": "Alex@Example.com", "tenant": "acme", "subject": "alex", "role": "administrator"
	}]})
}

fn parse(structure: &Value, people: Option<&Value>) -> Result<Inputs> {
	Inputs::parse(
		&structure.to_string(),
		people.map(Value::to_string).as_deref(),
	)
}

fn refusal(result: Result<Inputs>) -> String {
	match result {
		Err(Error::Invalid(message)) => message,
		other => panic!("expected an invalid input, got {other:?}"),
	}
}

#[test]
fn a_valid_pair_parses_and_lowercases_emails() {
	let inputs = parse(&structure(), Some(&people())).unwrap();
	let person = inputs.people_of("acme").next().unwrap();
	assert_eq!(person.email, "alex@example.com");
	assert_eq!(person.role.group(), "administrators");
	assert_eq!(inputs.structure.tenants[0].registry[0].id, "helper");
	assert_eq!(inputs.people_of("other").count(), 0);
}

#[test]
fn people_are_optional() {
	let inputs = parse(&structure(), None).unwrap();
	assert!(inputs.people.is_none());
	assert_eq!(inputs.people_of("acme").count(), 0);
}

#[rstest]
#[case::empty("")]
#[case::uppercase("Acme")]
#[case::digit_first("1acme")]
#[case::hyphen_first("-acme")]
#[case::underscore("ac_me")]
#[case::space("ac me")]
#[case::too_long("a23456789012345678901234567890123")]
fn a_bad_tenant_key_is_refused(#[case] key: &str) {
	let mut document = structure();
	document["tenants"][0]["key"] = json!(key);
	assert!(refusal(parse(&document, None)).contains("tenant key"));
}

#[rstest]
#[case::one("a")]
#[case::hyphens("acme-research-2")]
#[case::longest("a1234567890123456789012345678901")]
fn a_valid_tenant_key_is_accepted(#[case] key: &str) {
	let mut document = structure();
	document["tenants"][0]["key"] = json!(key);
	document["tenants"][0]["approvals"] = json!([]);
	assert!(parse(&document, None).is_ok());
}

#[test]
fn a_duplicate_tenant_key_is_refused() {
	let mut document = structure();
	let again = document["tenants"][0].clone();
	document["tenants"].as_array_mut().unwrap().push(again);
	assert!(refusal(parse(&document, None)).contains("appears twice"));
}

#[test]
fn a_blank_label_is_refused() {
	let mut document = structure();
	document["tenants"][0]["label"] = json!("  ");
	assert!(refusal(parse(&document, None)).contains("label"));
}

#[rstest]
#[case::subject(json!({"tenants": [{"key": "acme", "label": "A", "subjects": [{"id": "d", "groups": ["owners"]}]}]}))]
#[case::approval(json!({"tenants": [{"key": "acme", "label": "A", "approvals": [{"entry": {"id": "h", "version": "1"}, "groups": ["everyone"]}]}]}))]
fn an_unknown_group_is_refused(#[case] document: Value) {
	assert!(refusal(parse(&document, None)).contains("must be \"administrators\" or \"members\""));
}

#[test]
fn an_approval_without_a_group_is_refused() {
	let mut document = structure();
	document["tenants"][0]["approvals"][0]["groups"] = json!([]);
	assert!(refusal(parse(&document, None)).contains("names no group"));
}

#[test]
fn duplicate_subjects_entries_and_approvals_are_refused() {
	let mut document = structure();
	let subject = document["tenants"][0]["subjects"][0].clone();
	document["tenants"][0]["subjects"]
		.as_array_mut()
		.unwrap()
		.push(subject);
	assert!(refusal(parse(&document, None)).contains("subject \"dana\" appears twice"));

	let mut document = structure();
	let entry = document["tenants"][0]["registry"][0].clone();
	document["tenants"][0]["registry"]
		.as_array_mut()
		.unwrap()
		.push(entry);
	assert!(refusal(parse(&document, None)).contains("registry entry helper@1.0.0"));

	let mut document = structure();
	let approval = document["tenants"][0]["approvals"][0].clone();
	document["tenants"][0]["approvals"]
		.as_array_mut()
		.unwrap()
		.push(approval);
	assert!(refusal(parse(&document, None)).contains("approval of helper@1.0.0"));
}

#[test]
fn a_subject_with_whitespace_is_refused() {
	let mut document = structure();
	document["tenants"][0]["subjects"][0]["id"] = json!("da na");
	assert!(refusal(parse(&document, None)).contains("subject"));
}

#[rstest]
#[case::structure_field(json!({"tenants": [], "extra": 1}), None)]
#[case::tenant_field(json!({"tenants": [{"key": "a", "label": "A", "nodes": []}]}), None)]
#[case::person_field(structure(), Some(json!({"people": [{"email": "a@b.c", "tenant": "acme", "subject": "a", "role": "member", "admin": true}]})))]
fn an_unknown_field_is_refused(#[case] structure: Value, #[case] people: Option<Value>) {
	assert!(refusal(parse(&structure, people.as_ref())).contains("unknown field"));
}

#[test]
fn a_person_naming_an_unknown_tenant_is_refused() {
	let mut listed = people();
	listed["people"][0]["tenant"] = json!("nowhere");
	assert!(refusal(parse(&structure(), Some(&listed))).contains("not a structure key"));
}

#[test]
fn a_bad_role_is_refused() {
	let mut listed = people();
	listed["people"][0]["role"] = json!("owner");
	assert!(refusal(parse(&structure(), Some(&listed))).contains("unknown variant"));
}

#[rstest]
#[case::no_at("alex.example.com")]
#[case::two_ats("a@b@example.com")]
#[case::no_local("@example.com")]
#[case::no_domain("alex@")]
#[case::space("al ex@example.com")]
#[case::too_long(&format!("{}@example.com", "a".repeat(320)))]
fn a_bad_email_is_refused(#[case] email: &str) {
	let mut listed = people();
	listed["people"][0]["email"] = json!(email);
	assert!(refusal(parse(&structure(), Some(&listed))).contains("email"));
}

#[test]
fn the_same_email_twice_for_one_tenant_is_refused_whatever_its_case() {
	let mut listed = people();
	listed["people"].as_array_mut().unwrap().push(json!({
		"email": "alex@EXAMPLE.com", "tenant": "acme", "subject": "alex-2", "role": "member"
	}));
	assert!(refusal(parse(&structure(), Some(&listed))).contains("appears twice"));
}

#[test]
fn one_email_may_be_reserved_in_two_tenants() {
	let mut document = structure();
	let mut second = document["tenants"][0].clone();
	second["key"] = json!("globex");
	second["approvals"] = json!([]);
	document["tenants"].as_array_mut().unwrap().push(second);
	let mut listed = people();
	let mut other = listed["people"][0].clone();
	other["tenant"] = json!("globex");
	listed["people"].as_array_mut().unwrap().push(other);
	assert!(parse(&document, Some(&listed)).is_ok());
}

#[test]
fn malformed_json_names_the_input() {
	assert!(refusal(Inputs::parse("{", None)).starts_with("structure:"));
	assert!(refusal(Inputs::parse(&structure().to_string(), Some("[]"))).starts_with("people:"));
}

#[test]
fn the_digest_ignores_key_order_and_layout() {
	let reordered = r#" { "tenants" : [ { "approvals": [ { "groups": ["members"], "entry": { "version": "1.0.0", "id": "helper" } } ],
		"registry": [ { "description": {"en":"Helps"}, "name": {"en":"Helper"}, "kind":"tool", "version":"1.0.0", "id":"helper" } ],
		"subjects": [ { "groups": ["members"], "id": "dana" } ], "label": "Acme Research", "key": "acme" } ] } "#;
	let original = parse(&structure(), Some(&people())).unwrap();
	let again = Inputs::parse(reordered, Some(&people().to_string())).unwrap();
	assert_eq!(original.digest, again.digest);
	assert_eq!(original.digest.len(), 64);
	assert!(original.digest.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn the_digest_follows_the_content_and_the_presence_of_people() {
	let original = parse(&structure(), Some(&people())).unwrap().digest;
	let without = parse(&structure(), None).unwrap().digest;
	assert_ne!(original, without);

	let mut relabeled = structure();
	relabeled["tenants"][0]["label"] = json!("Acme Labs");
	assert_ne!(parse(&relabeled, Some(&people())).unwrap().digest, original);

	let mut renamed = people();
	renamed["people"][0]["subject"] = json!("alexandra");
	assert_ne!(
		parse(&structure(), Some(&renamed)).unwrap().digest,
		original
	);

	// Arrays are ordered: reordering them is a change.
	let mut two = structure();
	let mut second = two["tenants"][0].clone();
	second["key"] = json!("globex");
	second["approvals"] = json!([]);
	two["tenants"].as_array_mut().unwrap().push(second);
	let mut swapped = two.clone();
	swapped["tenants"].as_array_mut().unwrap().reverse();
	assert_ne!(
		parse(&two, None).unwrap().digest,
		parse(&swapped, None).unwrap().digest
	);
}

#[test]
fn canonical_json_sorts_keys_at_every_depth() {
	let value = json!({"b": [{"z": 1, "a": "x\"y"}], "a": null});
	assert_eq!(canonical(&value), r#"{"a":null,"b":[{"a":"x\"y","z":1}]}"#);
}
