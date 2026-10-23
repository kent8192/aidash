//! The way from a sending Node to a target (ADR 0064). A Node reaches the
//! other Nodes of its own Tenant, and the Nodes its Tenant Administrators
//! registered at it.
use crate::apps::identity::repositories::{node_registrations as registrations, nodes};
use crate::{Error, Result};
use aidash_domain::federation::route::{Route, Sender};
use aidash_domain::nodes::{DeploymentId, NodeAddress, NodeName, registration::Counterpart};
use reinhardt::db::backends::TransactionExecutor;

/// The sending Node, in the sender's Tenant scope. A named Node must belong to
/// the sender's Tenant: the name may come from a request (the dashboard's Node
/// selector) or a Run's chain, and a Node of another Tenant is refused as an
/// unknown Node is, so a sender cannot speak as a Node it does not hold. A
/// sender that names no Node is its Tenant's default Node: the one seam that
/// stands in until Workspaces carry their own Node.
pub(crate) async fn sending_node(
	tx: &mut dyn TransactionExecutor,
	sender: &Sender,
) -> Result<NodeName> {
	match &sender.node {
		Some(node) => {
			let holder = registrations::name_tenant(tx, node.as_str()).await?;
			if holder.as_deref() != Some(sender.tenant.as_str()) {
				return Err(unregistered());
			}
			Ok(node.clone())
		}
		None => Ok(nodes::default_of(tx, sender.tenant.as_str()).await?.name),
	}
}

fn sibling(sender: &Sender, from: NodeAddress, to: NodeAddress) -> Route {
	Route {
		tenant: sender.tenant.clone(),
		sender: from,
		subject: sender.subject.clone(),
		target: to,
		counterpart: Counterpart::SameDeployment,
		// Within a Tenant the policy decides, so its Nodes trust each other.
		atomic_transactions: true,
	}
}

/// The route to `target`, or `None` when the sender has none: the target is
/// itself, or is neither a Node of the sender's Tenant nor registered and
/// enabled at the sending Node. `lock` holds the registration until the
/// transaction ends, so a read that depends on it finishes before it changes.
pub(crate) async fn route(
	tx: &mut dyn TransactionExecutor,
	own: &DeploymentId,
	sender: &Sender,
	target: &NodeAddress,
	lock: bool,
) -> Result<Option<Route>> {
	let tenant = sender.tenant.as_str();
	let node = sending_node(tx, sender).await?;
	let from = own.node(node.clone());
	if *target == from {
		return Ok(None);
	}
	if target.deployment == *own
		&& registrations::name_tenant(tx, target.node.as_str())
			.await?
			.as_deref()
			== Some(tenant)
	{
		return Ok(Some(sibling(sender, from, target.clone())));
	}
	let Some(registration) =
		registrations::get_locked(tx, tenant, node.as_str(), target, lock).await?
	else {
		return Ok(None);
	};
	if !registration.enabled {
		return Ok(None);
	}
	Ok(Some(Route {
		tenant: sender.tenant.clone(),
		sender: from,
		subject: sender.subject.clone(),
		target: target.clone(),
		counterpart: registration.counterpart,
		atomic_transactions: registration.atomic_transactions,
	}))
}

/// [`route`] for a target given as text. A value that is no Node address names
/// no route.
pub(crate) async fn route_to(
	tx: &mut dyn TransactionExecutor,
	own: &DeploymentId,
	sender: &Sender,
	target: &str,
	lock: bool,
) -> Result<Option<Route>> {
	let Ok(target) = NodeAddress::parse(target) else {
		return Ok(None);
	};
	route(tx, own, sender, &target, lock).await
}

/// Every Node the sender addresses: the other Nodes of its Tenant, then the
/// Nodes registered and enabled at it.
pub(crate) async fn all(
	tx: &mut dyn TransactionExecutor,
	own: &DeploymentId,
	sender: &Sender,
	lock: bool,
) -> Result<Vec<Route>> {
	let tenant = sender.tenant.as_str();
	let node = sending_node(tx, sender).await?;
	let from = own.node(node.clone());
	let mut routes = vec![];
	for sibling_node in nodes::list(tx, tenant).await? {
		if sibling_node.name != node {
			routes.push(sibling(sender, from.clone(), own.node(sibling_node.name)));
		}
	}
	for registration in registrations::enabled_of(tx, tenant, node.as_str()).await? {
		if let Some(route) = route(tx, own, sender, &registration.source_node, lock).await? {
			routes.push(route);
		}
	}
	Ok(routes)
}

pub(crate) fn unregistered() -> Error {
	Error::Unauthorized
}

impl crate::authorization::access::Access {
	/// The route from the Node this authority acts for to `node`, read in its
	/// transaction and held there, so a read that depends on the registration
	/// finishes before the registration changes.
	pub(crate) async fn route_to(&mut self, node: &str) -> Result<Option<Route>> {
		let sender = Sender {
			tenant: aidash_domain::tenancy::TenantName::new(self.identity.tenant.clone())?,
			node: self.node.clone(),
			subject: Some(self.identity.subject.clone()),
		};
		let own = DeploymentId::parse(&self.node_id)?;
		route_to(&mut **self.tx, &own, &sender, node, true).await
	}
}
