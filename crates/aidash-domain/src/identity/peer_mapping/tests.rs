use super::*;
use rstest::{fixture, rstest};
#[fixture]
fn input() -> PeerMappingInput {
	PeerMappingInput {
		source_node: "aidash://remote".into(),
		source_tenant: "remote-tenant".into(),
		source_subject: "alice".into(),
		credential_id: Uuid::from_u128(1),
		enabled: true,
		expected_revision: 0,
	}
}
#[rstest]
#[case(-1)]
#[case(i64::MIN)]
#[case(i64::MAX)]
fn invalid_revisions_cannot_create_or_update_a_mapping(
	mut input: PeerMappingInput,
	#[case] revision: i64,
) {
	input.expected_revision = revision;
	assert!(
		matches!(input.validate("tenant","aidash://local"),Err(Error::Invalid(message)) if message=="invalid peer mapping or revision")
	);
}
#[rstest]
fn local_node_cannot_be_its_own_mapped_peer(mut input: PeerMappingInput) {
	input.source_node = "aidash://local".into();
	assert!(
		matches!(input.validate("tenant","aidash://local"),Err(Error::Invalid(message)) if message=="invalid peer mapping or revision")
	);
}
#[rstest]
#[case("tenant")]
#[case("source_tenant")]
#[case("source_subject")]
#[case("source_node")]
fn empty_identity_fields_fail_before_admission(mut input: PeerMappingInput, #[case] field: &str) {
	let tenant = if field == "tenant" { "" } else { "tenant" };
	match field {
		"source_tenant" => input.source_tenant.clear(),
		"source_subject" => input.source_subject.clear(),
		"source_node" => input.source_node.clear(),
		_ => {}
	}
	assert!(matches!(
		input.validate(tenant, "aidash://local"),
		Err(Error::Invalid(_))
	));
}
#[rstest]
fn revocation_requires_an_existing_revision(mut input: PeerMappingInput) {
	input.enabled = false;
	assert!(
		matches!(input.require_existing_revocation(),Err(Error::Invalid(message)) if message=="revocation requires an existing mapping")
	);
	input.expected_revision = 1;
	assert!(input.require_existing_revocation().is_ok());
	input.enabled = true;
	input.expected_revision = 0;
	assert!(input.require_existing_revocation().is_ok());
}
