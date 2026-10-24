use super::*;
use rstest::{fixture, rstest};

const LOCAL: &str = "aidash://local";

fn local() -> DeploymentId {
	DeploymentId::parse(LOCAL).unwrap()
}

#[fixture]
fn input() -> PeerMappingInput {
	PeerMappingInput {
		node: NodeName::parse("able-oak-abcd1234").unwrap(),
		source_node: NodeAddress::parse("aidash://remote/calm-heron-e5f6g7h8").unwrap(),
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
		matches!(input.validate("tenant",&local()),Err(Error::Invalid(message)) if message=="invalid peer mapping or revision")
	);
}
#[rstest]
fn a_node_cannot_be_its_own_mapped_peer(mut input: PeerMappingInput) {
	input.source_node = NodeAddress::parse(&format!("{LOCAL}/able-oak-abcd1234")).unwrap();
	assert!(
		matches!(input.validate("tenant",&local()),Err(Error::Invalid(message)) if message=="invalid peer mapping or revision")
	);
}
#[rstest]
fn another_node_of_this_deployment_is_a_valid_source(mut input: PeerMappingInput) {
	input.source_node = NodeAddress::parse(&format!("{LOCAL}/calm-heron-e5f6g7h8")).unwrap();
	assert!(input.validate("tenant", &local()).is_ok());
}
#[rstest]
#[case("tenant")]
#[case("source_subject")]
fn empty_identity_fields_fail_before_admission(mut input: PeerMappingInput, #[case] field: &str) {
	let tenant = if field == "tenant" { "" } else { "tenant" };
	if field == "source_subject" {
		input.source_subject.clear();
	}
	assert!(matches!(
		input.validate(tenant, &local()),
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
