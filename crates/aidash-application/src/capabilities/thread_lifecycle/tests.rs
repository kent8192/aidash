use super::*;
use async_trait::async_trait;
use rstest::rstest;
use std::collections::BTreeSet;

struct Fake {
	/// Actions the Tenant's policy allows, by `kind:action`.
	allowed: BTreeSet<String>,
	asked: Vec<String>,
}
#[async_trait]
impl DeletionAuthority for Fake {
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		let key = format!("{}:{action}", resource.kind);
		self.asked.push(key.clone());
		Ok(self.allowed.contains(&key))
	}
	async fn node_resource(&mut self, _thread: Uuid) -> Result<Resource> {
		Ok(Resource {
			tenant: "acme".into(),
			kind: "node".into(),
			id: "brave-otter-k3m9q2xa".into(),
			attributes: json!({}),
		})
	}
}
fn thread(owner: &str) -> Resource {
	Resource {
		tenant: "acme".into(),
		kind: "thread".into(),
		id: Uuid::nil().to_string(),
		attributes: json!({"owner": owner, "thread_id": Uuid::nil(), "duty_id": null}),
	}
}
fn fake(allowed: &[&str]) -> Fake {
	Fake {
		allowed: allowed.iter().map(|a| (*a).to_owned()).collect(),
		asked: vec![],
	}
}

#[rstest]
#[case::owner_with_delete("alice", &["thread:thread.delete"], true)]
#[case::owner_without_delete("alice", &[], false)]
#[case::reader_with_delete("bob", &["thread:thread.delete"], false)]
#[case::reader("bob", &["thread:thread.read"], false)]
#[case::node_administrator("bob", &["node:node.administer"], true)]
#[tokio::test]
async fn only_the_owner_or_a_node_administrator_deletes_a_thread(
	#[case] principal: &str,
	#[case] allowed: &[&str],
	#[case] expected: bool,
) {
	// Arrange: Alice owns the Thread.
	let mut scope = fake(allowed);
	// Act
	let permitted = may_delete(&mut scope, principal, &thread("alice"), Uuid::nil())
		.await
		.unwrap();
	// Assert
	assert_eq!(permitted, expected);
}

#[tokio::test]
async fn a_reader_holding_thread_delete_is_never_asked_for_it() {
	// Arrange: Bob reads Alice's Thread and holds `thread.delete` broadly.
	let mut scope = fake(&["thread:thread.delete", "thread:thread.read"]);
	// Act
	let permitted = may_delete(&mut scope, "bob", &thread("alice"), Uuid::nil())
		.await
		.unwrap();
	// Assert: only the Node administration decision counts for a non-owner.
	assert!(!permitted);
	assert_eq!(scope.asked, ["node:node.administer"]);
}
