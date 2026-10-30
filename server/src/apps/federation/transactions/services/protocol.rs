use crate::apps::federation::transactions::{
	LocalStatus, Manifest, Status, authority, participant,
};
use crate::authorization::identity::Actor;
use crate::{Error, Result, federation::Federation};
use aidash_application::federation::inbound::Admitted;
use aidash_application::transactions::management as application;
use aidash_domain::identity::execution::ExecutionPrincipal;
use reinhardt::{Depends, injectable};
use std::sync::Arc;
use tokio::sync::Semaphore;

use uuid::Uuid;

use crate::apps::federation::transactions::serializers::protocol::TransactionDetails;

#[derive(Clone)]
pub struct SubmissionCapacity {
	slots: Arc<Semaphore>,
}

#[injectable(scope = "singleton")]
pub async fn provide_submission_capacity() -> SubmissionCapacity {
	SubmissionCapacity {
		slots: Arc::new(Semaphore::new(8)),
	}
}

#[derive(Clone)]
pub struct Transactions {
	runtime: Federation,
	submissions: Arc<Semaphore>,
}

#[injectable(scope = "request")]
pub async fn provide(
	#[inject] runtime: Federation,
	#[inject] submissions: Depends<SubmissionCapacity>,
) -> Transactions {
	Transactions {
		runtime,
		submissions: submissions.slots.clone(),
	}
}

impl Transactions {
	/// The service for in-process Federation, which never submits.
	pub(crate) fn for_peers(runtime: Federation) -> Self {
		Self {
			runtime,
			submissions: Arc::new(Semaphore::new(1)),
		}
	}
	pub async fn submit(&self, actor: Actor, manifest: Manifest) -> Result<Status> {
		// Router clones share eight DI-scoped slots, leaving control capacity
		// for inbound preflights from independent Nodes.
		let _permit = self
			.submissions
			.acquire()
			.await
			.map_err(|_| Error::TransactionPending)?;
		application::submit(
			&crate::bootstrap::transaction_management_repository(&self.runtime),
			&principal(&actor)?,
			&manifest,
		)
		.await
		.map(Into::into)
		.map_err(Into::into)
	}

	pub async fn list(&self, actor: Actor) -> Result<Vec<Status>> {
		application::list(
			&crate::bootstrap::transaction_management_repository(&self.runtime),
			&principal(&actor)?,
		)
		.await
		.map(|rows| rows.into_iter().map(Into::into).collect())
		.map_err(Into::into)
	}
	pub async fn details(&self, actor: Actor, id: Uuid) -> Result<TransactionDetails> {
		application::details(
			&crate::bootstrap::transaction_management_repository(&self.runtime),
			&principal(&actor)?,
			id,
		)
		.await
		.map(Into::into)
		.map_err(Into::into)
	}
	pub async fn abort(&self, actor: Actor, id: Uuid) -> Result<Status> {
		application::abort(
			&crate::bootstrap::transaction_management_repository(&self.runtime),
			&principal(&actor)?,
			id,
		)
		.await
		.map(Into::into)
		.map_err(Into::into)
	}
	pub async fn participants(&self) -> Result<Vec<LocalStatus>> {
		application::participants(&crate::bootstrap::transaction_management_repository(
			&self.runtime,
		))
		.await
		.map(|rows| rows.into_iter().map(Into::into).collect())
		.map_err(Into::into)
	}
	pub(crate) async fn preflight(
		&self,
		admitted: Admitted,
		input: authority::Preflight,
	) -> Result<serde_json::Value> {
		authority::preflight(&self.runtime, &admitted, &input).await?;
		Ok(serde_json::json!({"authorized":true}))
	}
	pub(crate) async fn read_access(
		&self,
		admitted: Admitted,
		input: authority::Preflight,
	) -> Result<serde_json::Value> {
		authority::read_access(&self.runtime, &admitted, &input).await?;
		Ok(serde_json::json!({"authorized":true}))
	}
	pub(crate) async fn authority_ticket(
		&self,
		admitted: Admitted,
		id: Uuid,
	) -> Result<authority::Preflight> {
		authority::ticket(&self.runtime, &admitted, id).await
	}
	pub async fn reserve(&self, admitted: Admitted, manifest: Manifest) -> Result<LocalStatus> {
		let f = self.runtime.clone();
		require_atomic(&f, &admitted).await?;
		participant::reserve(&f, Some(&admitted), &admitted.sender.to_string(), &manifest).await
	}
	pub async fn prepare(&self, admitted: Admitted, manifest: Manifest) -> Result<LocalStatus> {
		let f = self.runtime.clone();
		participant::prepare(&f, Some(&admitted), &admitted.sender.to_string(), &manifest).await
	}
	pub async fn finish(&self, admitted: Admitted, manifest: Manifest) -> Result<LocalStatus> {
		let f = self.runtime.clone();
		participant::finish(&f, Some(&admitted), &admitted.sender.to_string(), &manifest).await
	}
	pub async fn decision(&self, admitted: Admitted, id: Uuid) -> Result<Status> {
		let sender = admitted.sender.to_string();
		let caller = Some(sender.as_str());
		application::decision(
			&crate::bootstrap::transaction_management_repository(&self.runtime),
			caller,
			id,
		)
		.await
		.map(Into::into)
		.map_err(Into::into)
	}
}

/// Transactions are Tenant content: a Deployment Administrator has no
/// principal here (#484).
fn principal(actor: &Actor) -> Result<ExecutionPrincipal> {
	match actor {
		Actor::DeploymentAdministrator => Err(Error::Forbidden),
		Actor::Subject(identity) => Ok(ExecutionPrincipal {
			credential_id: identity.credential_id,
			tenant: identity.tenant.clone(),
			subject: identity.subject.clone(),
		}),
	}
}

/// Taking part in a new atomic transaction is a Tenant's decision about the
/// sending Node (ADR 0064): its registration allows it, or the sender is
/// another Node of the same Tenant. Only a reservation asks: a participant that
/// admitted the transaction prepares, commits and aborts it even after the
/// registration changed, so recovery completes.
async fn require_atomic(f: &Federation, admitted: &Admitted) -> Result<()> {
	use crate::apps::identity::repositories::node_registrations as registrations;
	let tenancy = aidash_application::tenancy::TenancyScope::Tenant(admitted.tenant.clone());
	let allowed = f
		.store
		.pool
		.atomic(&tenancy, async |tx| {
			let registered = registrations::get(
				tx,
				admitted.tenant.as_str(),
				admitted.receiver.node.as_str(),
				&admitted.sender,
			)
			.await?;
			if let Some(registration) = registered {
				return Ok(registration.enabled && registration.atomic_transactions);
			}
			Ok(admitted.sender.deployment == f.config.deployment
				&& registrations::name_tenant(tx, admitted.sender.node.as_str())
					.await?
					.as_deref() == Some(admitted.tenant.as_str()))
		})
		.await?;
	if allowed {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}
