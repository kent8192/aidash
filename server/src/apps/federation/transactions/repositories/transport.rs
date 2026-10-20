//! Compose native route lookup with the bounded transaction adapter.
use crate::federation::Federation;
use aidash_application::Result;
use aidash_domain::{federation::route::Sender, transactions::Manifest};
use serde::{Serialize, de::DeserializeOwned};

/// A coordinator sends as its Node, and a participant as the Node it holds in
/// this Deployment, naming no subject.
pub(crate) async fn as_node(runtime: &Federation, node: &str) -> Result<Sender> {
	let address = aidash_domain::nodes::NodeAddress::parse(node)?;
	let tenant = runtime.node_tenant_of(&address.node).await?;
	Ok(Sender {
		tenant,
		node: Some(address.node),
		subject: None,
	})
}

/// The Node of this Deployment that takes part in `manifest`.
pub(crate) fn participant<'a>(runtime: &Federation, manifest: &'a Manifest) -> Option<&'a str> {
	manifest
		.participants
		.iter()
		.map(|participant| participant.node_id.as_str())
		.find(|node| {
			aidash_domain::nodes::NodeAddress::parse(node)
				.is_ok_and(|address| address.deployment == runtime.config.deployment)
		})
}

pub(crate) async fn remote<T: DeserializeOwned>(
	runtime: &Federation,
	sender: Sender,
	node: &str,
	method: &str,
	path: &str,
	body: Option<&impl Serialize>,
) -> Result<T> {
	crate::bootstrap::peer_transport(runtime)
		.transaction(
			async { runtime.route(&sender, node).await.map_err(Into::into) },
			method,
			path,
			body,
		)
		.await
}
