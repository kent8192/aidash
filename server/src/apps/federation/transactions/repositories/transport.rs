//! Compose native route lookup with the bounded transaction adapter.
use crate::federation::Federation;
use aidash_application::Result;
use aidash_domain::federation::route::Sender;
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

pub(crate) async fn remote<T: DeserializeOwned>(
	runtime: &Federation,
	sender: Sender,
	node: &str,
	method: &str,
	path: &str,
	body: Option<&impl Serialize>,
) -> Result<T> {
	let bytes = body
		.map(serde_json::to_vec)
		.transpose()?
		.unwrap_or_default();
	let request = aidash_application::federation::inbound::RequestDigest {
		method,
		path,
		body: &bytes,
	};
	crate::bootstrap::peer_transport(runtime)
		.transaction(
			async {
				runtime
					.route_delivery(&sender, node, &request)
					.await
					.map_err(Into::into)
			},
			method,
			path,
			body,
		)
		.await
}
