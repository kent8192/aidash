use super::*;
use rstest::rstest;

#[rstest]
#[case::plain("aidash://brave-otter")]
#[case::one_char("aidash://a")]
#[case::hundred(&format!("aidash://{}", "x".repeat(100)))]
fn deployment_ids_keep_the_settings_grammar(#[case] value: &str) {
	let id = DeploymentId::parse(value).unwrap();
	assert_eq!(id.to_string(), value);
	assert_eq!(id.name(), &value[9..]);
	assert!(value.len() <= DeploymentId::MAX_LEN);
}

#[rstest]
#[case::no_scheme("brave-otter")]
#[case::empty("aidash://")]
#[case::too_long(&format!("aidash://{}", "x".repeat(101)))]
#[case::slash("aidash://brave-otter/")]
#[case::node("aidash://brave-otter/able-oak-abcd1234")]
#[case::space("aidash://brave otter")]
fn deployment_ids_refuse_other_forms(#[case] value: &str) {
	assert!(DeploymentId::parse(value).is_err(), "{value}");
}

#[test]
fn node_names_are_drawn_like_tenant_names_and_parse_back() {
	let name = NodeName::from_entropy(*uuid::Uuid::new_v4().as_bytes());
	assert_eq!(NodeName::parse(name.as_str()).unwrap(), name);
	assert!(name.as_str().len() <= NodeName::MAX_LEN);
	let tenant = crate::identity::tenancy::TenantName::from_entropy([7; 16]);
	assert_eq!(NodeName::from_entropy([7; 16]).as_str(), tenant.as_str());
}

#[rstest]
#[case::upper("Able-oak-abcd1234")]
#[case::short_suffix("able-oak-abcd123")]
#[case::two_parts("able-abcd1234")]
#[case::four_parts("able-oak-tree-abcd1234")]
#[case::empty("")]
#[case::address("aidash://brave-otter/able-oak-abcd1234")]
fn node_names_refuse_other_forms(#[case] value: &str) {
	assert!(NodeName::parse(value).is_err(), "{value}");
}

#[test]
fn node_addresses_round_trip() {
	let address = NodeAddress::parse("aidash://brave-otter/able-oak-abcd1234").unwrap();
	assert_eq!(address.deployment, "aidash://brave-otter");
	assert_eq!(address.node, "able-oak-abcd1234");
	assert_eq!(
		address.to_string(),
		"aidash://brave-otter/able-oak-abcd1234"
	);
	assert_eq!(
		address.deployment.node(address.node.clone()).to_string(),
		address.to_string()
	);
	let json = serde_json::to_string(&address).unwrap();
	assert_eq!(json, "\"aidash://brave-otter/able-oak-abcd1234\"");
	assert_eq!(serde_json::from_str::<NodeAddress>(&json).unwrap(), address);
	assert_eq!(
		NodeAddress::MAX_LEN,
		DeploymentId::MAX_LEN + 1 + NodeName::MAX_LEN
	);
}

#[rstest]
#[case::deployment_only("aidash://brave-otter")]
#[case::trailing_slash("aidash://brave-otter/able-oak-abcd1234/")]
#[case::trailing_path("aidash://brave-otter/able-oak-abcd1234/agents")]
#[case::tenant_segment("aidash://brave-otter/acme/able-oak-abcd1234")]
#[case::bad_node("aidash://brave-otter/node")]
#[case::bad_deployment("aidash://brave otter/able-oak-abcd1234")]
fn node_addresses_refuse_other_forms(#[case] value: &str) {
	assert!(NodeAddress::parse(value).is_err(), "{value}");
}

#[test]
fn key_ids_digest_the_raw_public_key() {
	let id = DeploymentKeyId::of_public_key(&[0; 32]);
	assert_eq!(id.as_str().len(), DeploymentKeyId::LEN);
	assert_eq!(DeploymentKeyId::parse(id.as_str()).unwrap(), id);
	assert_ne!(id, DeploymentKeyId::of_public_key(&[1; 32]));
	assert!(DeploymentKeyId::parse("short").is_err());
	assert!(DeploymentKeyId::parse(&format!("{}=", &id.as_str()[..42])).is_err());
}
