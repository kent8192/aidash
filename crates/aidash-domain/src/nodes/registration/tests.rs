use super::*;
use rstest::rstest;

const OWN: &str = "aidash://brave-otter";
const OWNER: &str = "able-oak-abcd1234";
const KEY: &str = "kJ2hQx0aEoV4hB1CqVwMr4mP3oY8pQZLQ0wU6o3m1sA";

fn own() -> DeploymentId {
	DeploymentId::parse(OWN).unwrap()
}

fn owner() -> NodeName {
	NodeName::parse(OWNER).unwrap()
}

fn input(source: &str, endpoint: Option<&str>, key: Option<&str>) -> RegistrationInput {
	RegistrationInput {
		source_node: NodeAddress::parse(source).unwrap(),
		endpoint: endpoint.map(str::to_owned),
		dashboard_origin: None,
		key_id: key.map(|key| DeploymentKeyId::parse(key).unwrap()),
		atomic_transactions: false,
		enabled: true,
		expected_revision: 0,
	}
}

#[test]
fn a_node_of_this_deployment_pins_nothing() {
	let request = input("aidash://brave-otter/calm-heron-e5f6g7h8", None, None);
	assert_eq!(
		request.requested(&own(), &owner()).unwrap(),
		Requested::SameDeployment
	);
}

#[rstest]
#[case::endpoint(Some("https://peer.example.com"), None)]
#[case::key(None, Some(KEY))]
#[case::both(Some("https://peer.example.com"), Some(KEY))]
fn a_node_of_this_deployment_refuses_an_endpoint_or_a_key_id(
	#[case] endpoint: Option<&str>,
	#[case] key: Option<&str>,
) {
	let request = input("aidash://brave-otter/calm-heron-e5f6g7h8", endpoint, key);
	assert!(request.requested(&own(), &owner()).is_err());
}

#[test]
fn a_node_never_registers_itself() {
	let request = input("aidash://brave-otter/able-oak-abcd1234", None, None);
	assert!(request.requested(&own(), &owner()).is_err());
}

#[test]
fn a_node_of_another_deployment_needs_both_its_endpoint_and_its_key_id() {
	let source = "aidash://calm-heron/able-oak-abcd1234";
	let ok = input(source, Some("https://peer.example.com/"), Some(KEY));
	assert_eq!(
		ok.requested(&own(), &owner()).unwrap(),
		Requested::OtherDeployment {
			endpoint: "https://peer.example.com".into(),
			key_id: DeploymentKeyId::parse(KEY).unwrap(),
		}
	);
	assert!(
		input(source, None, Some(KEY))
			.requested(&own(), &owner())
			.is_err()
	);
	assert!(
		input(source, Some("https://peer.example.com"), None)
			.requested(&own(), &owner())
			.is_err()
	);
	assert!(
		input(source, Some("ftp://peer.example.com"), Some(KEY))
			.requested(&own(), &owner())
			.is_err(),
		"an endpoint that is not HTTP(S)"
	);
}

#[test]
fn a_remote_preview_origin_must_be_an_origin_and_is_not_pinned_for_local_nodes() {
	let source = "aidash://calm-heron/able-oak-abcd1234";
	let mut request = input(source, Some("https://peer.example.com"), Some(KEY));
	request.dashboard_origin = Some("https://dashboard.example.com".into());
	assert!(request.requested(&own(), &owner()).is_ok());
	for origin in ["https://dashboard.example.com/path", "javascript:alert(1)"] {
		request.dashboard_origin = Some(origin.into());
		assert!(request.requested(&own(), &owner()).is_err(), "{origin}");
	}
	let mut local = input("aidash://brave-otter/calm-heron-e5f6g7h8", None, None);
	local.dashboard_origin = Some("https://dashboard.example.com".into());
	assert!(local.requested(&own(), &owner()).is_err());
}

#[rstest]
#[case::negative(-1, true, false)]
#[case::creating(0, true, true)]
#[case::replacing(3, true, true)]
#[case::disabling_nothing(0, false, false)]
#[case::disabling_one(3, false, true)]
fn revisions_and_disabling(#[case] revision: i64, #[case] enabled: bool, #[case] valid: bool) {
	let mut request = input("aidash://brave-otter/calm-heron-e5f6g7h8", None, None);
	request.expected_revision = revision;
	request.enabled = enabled;
	assert_eq!(request.requested(&own(), &owner()).is_ok(), valid);
}

#[test]
fn disabling_a_node_of_another_deployment_asks_nothing_of_it() {
	let mut request = input("aidash://calm-heron/able-oak-abcd1234", None, None);
	request.enabled = false;
	request.expected_revision = 2;
	assert_eq!(
		request.requested(&own(), &owner()).unwrap(),
		Requested::Keep
	);
}

#[test]
fn the_three_columns_are_all_set_or_all_null() {
	assert_eq!(
		Counterpart::from_columns(None, None, None).unwrap(),
		Counterpart::SameDeployment
	);
	let other = Counterpart::from_columns(
		Some("https://peer.example.com".into()),
		Some("pem".into()),
		Some(KEY.into()),
	)
	.unwrap();
	assert_eq!(
		other.columns(),
		(Some("https://peer.example.com"), Some("pem"), Some(KEY))
	);
	for partial in [
		(Some("https://peer.example.com".to_owned()), None, None),
		(None, Some("pem".to_owned()), None),
		(None, None, Some(KEY.to_owned())),
		(
			Some("https://peer.example.com".to_owned()),
			Some("pem".to_owned()),
			None,
		),
	] {
		assert!(Counterpart::from_columns(partial.0, partial.1, partial.2).is_err());
	}
	assert!(
		Counterpart::from_columns(Some("e".into()), Some("p".into()), Some("short".into()))
			.is_err()
	);
}
