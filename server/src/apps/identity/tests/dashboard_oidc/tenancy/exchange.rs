//! Federation between Nodes of one Deployment (#241): the same acceptance that
//! decides a request off the wire decides it in process, and the sending Node
//! a request names must be one its Tenant holds.
use super::*;
use aidash_domain::federation::route::Sender;

/// What an in-process request got: `Ok` when an operation answered, otherwise
/// the status the receiving Node refused it with, or "no route" when the
/// sender has no way to the target at all.
#[derive(Debug, PartialEq, Eq)]
enum Reached {
	/// The sender addresses no such Node.
	NoRoute,
	/// Acceptance refused the sender.
	Refused(u16),
	/// Acceptance passed and the operation itself answered `status`.
	Accepted(u16),
}

impl Deployment {
	/// A `POST /scoped/discover` from `sender` to `target`. A caller that acts
	/// as the Node itself passes acceptance without a mapping, and the
	/// operation then refuses it because discovery reads a subject's data: 403
	/// is "accepted", 401 is "not accepted".
	async fn exchange(&self, sender: &Sender, target: &str) -> Reached {
		let result = self
			.federation
			.request::<Value>(
				sender,
				target,
				reqwest::Method::POST,
				"/scoped/discover",
				Some(&json!({"search":{}})),
			)
			.await;
		let Err(error) = result else {
			return Reached::Accepted(200);
		};
		let text = error.to_string();
		if matches!(error, aidash_server::Error::Unauthorized) {
			return Reached::NoRoute;
		}
		let status = |code: u16| text.contains(&format!("returned {code}"));
		if status(401) {
			Reached::Refused(401)
		} else if status(403) {
			Reached::Accepted(403)
		} else {
			panic!("unexpected exchange outcome: {text}")
		}
	}

	pub(super) async fn nodes_tenant(&self, label: &str) -> (Person, String, String, String) {
		let person = self.person(&label.to_lowercase()).await;
		let (tenant, context) = self.node_tenant(&person, label).await;
		let address = self.address_of(&person, &context, &tenant).await;
		(person, tenant, context, address)
	}

	fn node_of(address: &str) -> aidash_domain::nodes::NodeName {
		aidash_domain::nodes::NodeAddress::parse(address)
			.unwrap()
			.node
	}

	fn sender(tenant: &str, node: Option<&str>) -> Sender {
		Sender {
			tenant: aidash_domain::tenancy::TenantName::new(tenant).unwrap(),
			node: node.map(Self::node_of),
			subject: None,
		}
	}

	/// `tenant` registers `source` at its default Node.
	pub(super) async fn accepts(&self, person: &Person, context: &str, tenant: &str, source: &str) {
		let receiving = self.default_node(person, context, tenant).await;
		let (status, body) = self
			.register(
				person,
				context,
				tenant,
				&receiving,
				json!({"source_node":source,"atomic_transactions":false,"enabled":true,"expected_revision":0}),
			)
			.await;
		assert_eq!(status, 200, "{body}");
	}
}

#[rstest::rstest]
#[tokio::test]
async fn another_tenants_node_is_reached_only_through_registrations_on_both_sides(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let (alice, alice_tenant, alice_context, alice_node) = d.nodes_tenant("Alice").await;
	let (bob, bob_tenant, bob_context, bob_node) = d.nodes_tenant("Bob").await;
	let from_bob = Deployment::sender(&bob_tenant, None);

	// Bob's Node addresses nothing until Bob registers Alice's.
	assert_eq!(d.exchange(&from_bob, &alice_node).await, Reached::NoRoute);

	// Bob registers Alice's Node, which gives Bob a route; Alice has not
	// registered Bob's, so Alice's Node refuses the request before any operation.
	d.accepts(&bob, &bob_context, &bob_tenant, &alice_node)
		.await;
	assert_eq!(
		d.exchange(&from_bob, &alice_node).await,
		Reached::Refused(401)
	);

	// Alice registers Bob's Node: the request is accepted as Bob's Node, and
	// the operation refuses it because it names no subject with a Peer Mapping.
	d.accepts(&alice, &alice_context, &alice_tenant, &bob_node)
		.await;
	assert_eq!(
		d.exchange(&from_bob, &alice_node).await,
		Reached::Accepted(403)
	);

	// Disabling the registration refuses the sender again.
	let receiving = d.default_node(&alice, &alice_context, &alice_tenant).await;
	let (status, body) = d
		.register(
			&alice,
			&alice_context,
			&alice_tenant,
			&receiving,
			json!({"source_node":bob_node,"atomic_transactions":false,"enabled":false,"expected_revision":1}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(
		d.exchange(&from_bob, &alice_node).await,
		Reached::Refused(401)
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn two_nodes_of_one_tenant_need_no_registration_and_it_works_without_a_deployment_key(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::without_key(runtime, open_rules()).await;
	let (alice, tenant, context, default) = d.nodes_tenant("Alice").await;
	assert!(d.federation.config.deployment_key.is_none());
	let (status, second) = d.create_node(&alice, &context, &tenant, "Second").await;
	assert_eq!(status, 201, "{second}");
	let second = second["address"].as_str().unwrap().to_owned();

	assert_eq!(
		d.exchange(&Deployment::sender(&tenant, None), &second)
			.await,
		Reached::Accepted(403)
	);
	assert_eq!(
		d.exchange(&Deployment::sender(&tenant, Some(&second)), &default)
			.await,
		Reached::Accepted(403)
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_sender_names_only_a_node_its_tenant_holds(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let (alice, alice_tenant, alice_context, alice_node) = d.nodes_tenant("Alice").await;
	let (bob, bob_tenant, bob_context, bob_node) = d.nodes_tenant("Bob").await;
	let (status, second) = d
		.create_node(&alice, &alice_context, &alice_tenant, "Second")
		.await;
	assert_eq!(status, 201, "{second}");
	let alice_second = second["address"].as_str().unwrap().to_owned();
	d.accepts(&bob, &bob_context, &bob_tenant, &alice_node)
		.await;
	d.accepts(&alice, &alice_context, &alice_tenant, &bob_node)
		.await;

	// A sender naming Alice's own non-default Node is accepted.
	assert_eq!(
		d.exchange(
			&Deployment::sender(&alice_tenant, Some(&alice_second)),
			&alice_node
		)
		.await,
		Reached::Accepted(403)
	);
	// Bob naming Alice's Node as the sending Node would take the in-Tenant path
	// to Alice's other Node, or speak as Alice's Node across Tenants: both are
	// refused as an unknown Node is.
	assert_eq!(
		d.exchange(
			&Deployment::sender(&bob_tenant, Some(&alice_node)),
			&alice_second
		)
		.await,
		Reached::NoRoute
	);
	assert_eq!(
		d.exchange(
			&Deployment::sender(&bob_tenant, Some(&alice_node)),
			&bob_node
		)
		.await,
		Reached::NoRoute
	);
	assert_eq!(
		d.exchange(
			&Deployment::sender(&bob_tenant, Some(&alice_second)),
			&alice_node
		)
		.await,
		Reached::NoRoute
	);
	d.finish().await;
}
