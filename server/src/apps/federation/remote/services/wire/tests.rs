use super::*;
use aidash_domain::{
	federation::route::Route,
	nodes::{DeploymentId, NodeAddress, registration::Counterpart},
	tenancy::TenantName,
};
use rstest::rstest;

/// A Node of this Deployment, reached in process.
fn sibling() -> Route {
	Route {
		tenant: TenantName::new("acme").unwrap(),
		sender: NodeAddress::parse("aidash://home/quiet-otter-cd34ef56").unwrap(),
		subject: None,
		target: NodeAddress::parse("aidash://home/calm-heron-ab12cd34").unwrap(),
		counterpart: Counterpart::SameDeployment,
		atomic_transactions: false,
	}
}

/// A transport that holds no runtime, as in a process that never built a `Federation`.
fn without_runtime() -> Wire {
	Wire {
		http: HttpTransport {
			endpoints: Endpoints::new(Default::default()).unwrap(),
			deployment: DeploymentId::parse("aidash://home").unwrap(),
			egress: Default::default(),
		},
		runtime: None,
		silent: Default::default(),
	}
}

fn input() -> Input {
	Input {
		reference: aidash_domain::federation::dependencies::Reference::Registry {
			node_id: "aidash://home/calm-heron-ab12cd34".into(),
			id: "agent".into(),
			version: "1.0.0".into(),
			digest: "approved".into(),
		},
	}
}

#[rstest]
#[tokio::test]
async fn a_sibling_node_is_not_asked_without_a_runtime() {
	let wire = without_runtime();
	let route = sibling();
	assert!(matches!(
		wire.check(&route, &input(), Duration::from_secs(20)).await,
		Check::NotAsked
	));
	assert_eq!(wire.verify(&route, &[]).await, Verification::NotAsked);
}

#[rstest]
#[tokio::test]
async fn an_authority_with_no_transport_asks_nobody() {
	let reach = Reach(None, Hold::default());
	let route = sibling();
	assert!(matches!(
		reach.check(&route, &input(), Duration::from_secs(20)).await,
		Check::NotAsked
	));
	assert_eq!(reach.verify(&route, &[]).await, Verification::NotAsked);
}

#[rstest]
#[tokio::test]
async fn a_transport_without_a_runtime_still_reaches_other_deployments() {
	// Another Deployment goes over HTTP, so a missing runtime does not decide it.
	let mut route = sibling();
	route.counterpart = Counterpart::OtherDeployment {
		endpoint: "https://foreign.invalid".into(),
		key: aidash_domain::nodes::registration::PinnedKey {
			key_id: aidash_domain::nodes::DeploymentKeyId::parse(&"a".repeat(43)).unwrap(),
			public_key_pem: "pem".into(),
		},
	};
	let wire = without_runtime();
	// Federation is off in this transport, which is a refusal and not a failure.
	assert_eq!(wire.verify(&route, &[]).await, Verification::Rejected);
	assert!(matches!(
		wire.check(&route, &input(), Duration::from_secs(20)).await,
		Check::Refused
	));
}
