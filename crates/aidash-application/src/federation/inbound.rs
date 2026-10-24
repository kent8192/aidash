//! One function decides every inbound Federation exchange, whatever carried it
//! (ADR 0064). Every step fails closed.
use crate::{
	Error, Result,
	ports::federation::inbound::{InboundRepository, ReceivingKeyOwner},
};
use aidash_capability::federation::RequestBinding;
use aidash_domain::{
	federation::protocol::CURRENT,
	nodes::{DeploymentId, NodeAddress, registration::Counterpart},
	tenancy::TenantName,
};
use uuid::Uuid;

/// What the transport knows before anything is checked. The receiver, sender
/// and subject of a token are the unverified claims; `accept` verifies them.
#[derive(Debug, Clone)]
pub struct Inbound {
	pub receiver: NodeAddress,
	pub sender: NodeAddress,
	/// The sending subject's key in the sending Tenant's policy, or empty when
	/// the request acts as the sending Node itself.
	pub subject: String,
	pub proof: Proof,
}

/// What stands behind an inbound request.
#[derive(Debug, Clone)]
pub enum Proof {
	/// Built by `LocalTransport` for an exchange inside this process, and by
	/// nothing else: the sender and subject are what the server already
	/// authenticated.
	InProcess,
	/// A compact Federation token, not yet verified.
	Token(String),
}

/// The request a token is bound to.
#[derive(Debug, Clone, Copy)]
pub struct RequestDigest<'a> {
	pub method: &'a str,
	/// The path under `/federation/v0.1` with its query.
	pub path: &'a str,
	pub body: &'a [u8],
}

/// How the sender acts at the receiving Node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Acting {
	/// A subject of the receiving Tenant's own policy: the policy decides, and
	/// `node.use` gates the Node.
	SameTenant { subject: String },
	/// A subject of another Tenant, mapped to a local credential.
	Mapped {
		credential_id: Uuid,
		subject: String,
	},
	/// The sending Node itself, with no subject: a callback that a grant or a
	/// transaction authorizes, never a read of the Tenant's data on a subject's
	/// behalf.
	Node,
}

/// An inbound request that passed every check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admitted {
	/// The receiving Node's Tenant.
	pub tenant: TenantName,
	pub receiver: NodeAddress,
	pub sender: NodeAddress,
	pub acting: Acting,
}

impl Admitted {
	/// The subject the request acts for, if it names one.
	pub fn subject(&self) -> Option<&str> {
		match &self.acting {
			Acting::SameTenant { subject } | Acting::Mapped { subject, .. } => Some(subject),
			Acting::Node => None,
		}
	}
}

/// Every refusal up to and including the signature answers the same
/// `Unauthorized`, so a caller cannot tell a wrong Node name from a wrong key.
pub async fn accept(
	inbound: Inbound,
	request: &RequestDigest<'_>,
	repository: &dyn InboundRepository,
) -> Result<Admitted> {
	let Inbound {
		receiver,
		sender,
		subject,
		proof,
	} = inbound;
	let own = repository.deployment();
	// 1. The request is for a Node of this Deployment.
	if receiver.deployment != *own {
		return Err(Error::Unauthorized);
	}
	// 2. The receiving Node names a Tenant.
	let tenant = repository
		.node_tenant(&receiver.node)
		.await?
		.ok_or(Error::Unauthorized)?;
	// 3. The Tenant is not deleted: its Nodes refuse Federation (ADR 0060).
	if !repository.tenant_live(&tenant).await? {
		return Err(Error::Unauthorized);
	}
	// 4. The sender.
	let sender_tenant = match (&proof, sender.deployment == *own) {
		(Proof::InProcess, true) => {
			let holder = repository
				.node_tenant(&sender.node)
				.await?
				.ok_or(Error::Unauthorized)?;
			if !repository.tenant_live(&holder).await? {
				return Err(Error::Unauthorized);
			}
			Some(holder)
		}
		(Proof::Token(_), false) => {
			if !repository.federation_enabled() || !repository.holds_key() {
				return Err(Error::Unauthorized);
			}
			None
		}
		// A sender of this Deployment over HTTP, or a token carried in process.
		_ => return Err(Error::Unauthorized),
	};
	// 5. Within a Tenant, no registration or mapping: the policy decides.
	if sender_tenant.as_ref() == Some(&tenant) {
		if subject.is_empty() {
			return Ok(Admitted {
				tenant,
				receiver,
				sender,
				acting: Acting::Node,
			});
		}
		// 7. Policy.
		if !repository
			.subject_may_use(&tenant, &receiver.node, &subject)
			.await?
		{
			return Err(Error::Forbidden);
		}
		return Ok(Admitted {
			tenant,
			receiver,
			sender,
			acting: Acting::SameTenant { subject },
		});
	}
	// 6. Another Tenant: a Node Registration, and for a Node of another
	// Deployment a signature that verifies with the key it pinned.
	let registration = repository
		.registration(&tenant, &receiver.node, &sender)
		.await?
		.filter(|registration| registration.enabled)
		.ok_or(Error::Unauthorized)?;
	let owner = ReceivingKeyOwner::Registration {
		tenant: tenant.clone(),
		node: receiver.node.clone(),
		source_node: sender.clone(),
	};
	let mut advancement = None;
	match (&proof, &registration.counterpart) {
		(Proof::InProcess, Counterpart::SameDeployment) => {}
		(Proof::Token(raw), Counterpart::OtherDeployment { .. }) => {
			let stored = repository
				.receiving_keys(&owner)
				.await?
				.ok_or(Error::Unauthorized)?;
			let (claims, candidate) = super::registrations::verify_request(
				raw,
				&sender.deployment,
				&stored.keys,
				repository.now(),
				&RequestBinding {
					protocol: CURRENT,
					method: request.method,
					path: request.path,
					body: request.body,
				},
			)?;
			advancement = candidate.map(|keys| (stored.keys_revision, keys));
			if claims.iss != sender.deployment.as_str()
				|| claims.sub != sender.to_string()
				|| claims.aud != receiver.to_string()
				|| claims.subject != subject
			{
				return Err(Error::Unauthorized);
			}
		}
		_ => return Err(Error::Unauthorized),
	}
	let acting = if subject.is_empty() {
		Acting::Node
	} else {
		// A missing mapping after authentication is a denial. No trust write is
		// performed until this last admission check has passed.
		let mapping = repository
			.mapping(&tenant, &receiver.node, &sender, &subject)
			.await?
			.filter(|mapping| mapping.enabled)
			.ok_or(Error::Forbidden)?;
		Acting::Mapped {
			credential_id: mapping.credential_id,
			subject,
		}
	};
	if let Some((keys_revision, keys)) = advancement {
		if !repository
			.advance_keys(&owner, keys_revision, registration.revision, &keys)
			.await?
		{
			return Err(Error::Unauthorized);
		}
	}
	Ok(Admitted {
		tenant,
		receiver,
		sender,
		acting,
	})
}

/// A Deployment Administrator of another Deployment reading a Tenant's graph
/// (ADR 0043): the one inbound caller that is not a Node.
#[derive(Debug, Clone)]
pub struct ViewerInbound {
	pub receiver: DeploymentId,
	pub viewer: DeploymentId,
	/// The Tenant the viewer names; a Deployment Administrator selects one.
	pub tenant: TenantName,
	/// The viewer's browser identity, from `oidc:<uuid>` in the token.
	pub identity: Uuid,
	pub token: String,
}

/// An admitted viewer, with the grant revision its projection binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedViewer {
	pub tenant: TenantName,
	pub viewer: DeploymentId,
	pub identity: Uuid,
	pub grant_revision: i64,
}

pub async fn accept_grant_viewer(
	inbound: ViewerInbound,
	request: &RequestDigest<'_>,
	repository: &dyn InboundRepository,
) -> Result<AdmittedViewer> {
	let ViewerInbound {
		receiver,
		viewer,
		tenant,
		identity,
		token,
	} = inbound;
	if !repository.federation_enabled()
		|| !repository.holds_key()
		|| receiver != *repository.deployment()
		|| viewer == *repository.deployment()
		|| !repository.tenant_live(&tenant).await?
	{
		return Err(Error::Unauthorized);
	}
	let grant = repository
		.viewer_grant(&tenant, &viewer, identity)
		.await?
		.ok_or(Error::Unauthorized)?;
	let owner = ReceivingKeyOwner::ViewerGrant {
		tenant: tenant.clone(),
		viewer: viewer.clone(),
		identity,
	};
	let stored = repository
		.receiving_keys(&owner)
		.await?
		.ok_or(Error::Unauthorized)?;
	let (claims, candidate) = super::registrations::verify_request(
		&token,
		&viewer,
		&stored.keys,
		repository.now(),
		&RequestBinding {
			protocol: CURRENT,
			method: request.method,
			path: request.path,
			body: request.body,
		},
	)?;
	if claims.iss != viewer.as_str()
		|| claims.sub != viewer.as_str()
		|| claims.aud != receiver.as_str()
		|| claims.subject != format!("oidc:{identity}")
	{
		return Err(Error::Unauthorized);
	}
	if let Some(keys) = candidate {
		if !repository
			.advance_keys(&owner, stored.keys_revision, grant.revision, &keys)
			.await?
		{
			return Err(Error::Unauthorized);
		}
	}
	Ok(AdmittedViewer {
		tenant,
		viewer,
		identity,
		grant_revision: grant.revision,
	})
}

#[cfg(test)]
mod tests;
