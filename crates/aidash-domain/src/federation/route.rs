//! Where a Federation request goes and as whom it is sent (ADR 0064).
use crate::nodes::{NodeAddress, NodeName, registration::Counterpart};
use crate::tenancy::TenantName;

/// Who a request is sent as: one of the sending Tenant's Nodes and, when the
/// request acts for a subject of that Tenant, that subject. A request that
/// acts as the Node itself, such as a grant callback, names no subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sender {
	pub tenant: TenantName,
	/// The sending Node. `None` is the Tenant's default Node, which stands in
	/// wherever a Run or Workspace does not yet name its Node.
	pub node: Option<NodeName>,
	pub subject: Option<String>,
}

/// A request's way to one Node: the sender and the target Node, with what the
/// sender's Node Registration of the target pins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
	/// The sending Node's Tenant.
	pub tenant: TenantName,
	/// The sending Node.
	pub sender: NodeAddress,
	pub subject: Option<String>,
	pub target: NodeAddress,
	pub counterpart: Counterpart,
	/// Whether the registered Node may take part in an atomic transaction with
	/// the sending Node.
	pub atomic_transactions: bool,
}

impl Route {
	/// Whether the target is a Node of this Deployment, reached in process.
	pub fn local(&self) -> bool {
		matches!(self.counterpart, Counterpart::SameDeployment)
	}
}
