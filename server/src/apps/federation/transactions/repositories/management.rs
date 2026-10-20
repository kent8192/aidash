//! Native records and authority adapters for transaction management use cases.
use super::coordination::plumbing;
use crate::apps::federation::transactions::models::{AtomicParticipant, coordinator_records};
use crate::{Error, federation::Federation};
use aidash_application::{
	Result,
	ports::transactions::authority::AuthorityRepository,
	ports::transactions::management::ManagementRepository,
	transactions::{admission, authority::control},
};
use aidash_domain::{
	identity::execution::ExecutionPrincipal,
	transactions::{
		Manifest,
		authority::{Origin, Status},
		coordination::{LocalStatus, Vote},
		management::History,
	},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub(crate) struct Repository {
	pub runtime: Federation,
}

#[async_trait]
impl ManagementRepository for Repository {
	async fn submit_deployment_administrator(&self, manifest: &Manifest) -> Result<Status> {
		admission::submit(
			&crate::bootstrap::transaction_admission_repository(&self.runtime),
			manifest,
			None,
		)
		.await
	}
	async fn submit_subject(
		&self,
		identity: &ExecutionPrincipal,
		manifest: &Manifest,
	) -> Result<Status> {
		let repository = crate::bootstrap::transaction_authority_repository(&self.runtime);
		// A preflight releases its disclosure locks before the complete manifest
		// can be sent. Suspend new cross-node scoped submissions until the
		// disclosure protocol binds revocation to delivery. Existing durable
		// transactions remain available to the recovery coordinator.
		if manifest.participants.iter().any(|participant| {
			!aidash_domain::nodes::same_deployment(
				&participant.node_id,
				self.runtime.config.deployment.as_str(),
			)
		}) && AuthorityRepository::origin(&repository, manifest.id)
			.await?
			.is_none()
		{
			return Err(aidash_application::Error::Forbidden);
		}
		control::submit(&repository, identity, manifest).await
	}
	async fn candidates(
		&self,
		identity: Option<&ExecutionPrincipal>,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<Status>> {
		plumbing(&self.runtime, async |tx| {
			coordinator_records::visible_candidates(tx, identity, cursor).await
		})
		.await
		.map(|rows| rows.into_iter().map(Into::into).collect())
		.map_err(Into::into)
	}
	async fn require_owner(&self, identity: &ExecutionPrincipal, id: Uuid) -> Result<()> {
		control::require_owner(
			&crate::bootstrap::transaction_authority_repository(&self.runtime),
			identity,
			id,
		)
		.await
		.map(|_: Origin| ())
	}
	async fn authorize(
		&self,
		identity: &ExecutionPrincipal,
		state: &Status,
		action: &str,
	) -> Result<()> {
		control::manage(
			&crate::bootstrap::transaction_authority_repository(&self.runtime),
			identity,
			state,
			action,
		)
		.await
		.map_err(|error| {
			// The former HTTP adapter suppressed native External errors on list
			// disclosure. Preserve that category without erasing database identity.
			match Error::from(error) {
				Error::External(message) => aidash_application::Error::External(message),
				error => error.into(),
			}
		})
	}
	async fn status(&self, id: Uuid) -> Result<Status> {
		crate::bootstrap::transaction_coordinator(&self.runtime)
			.status(id)
			.await
	}
	async fn history(&self, id: Uuid) -> Result<(Vec<Vote>, Vec<History>)> {
		Ok(plumbing(&self.runtime, async |db| {
			let votes = coordinator_records::votes(db, id)
				.await?
				.into_iter()
				.map(Into::into)
				.collect();
			let history = coordinator_records::audit(db, id)
				.await?
				.into_iter()
				.map(Into::into)
				.collect();
			Ok((votes, history))
		})
		.await?)
	}
	async fn abort_deployment_administrator(&self, id: Uuid) -> Result<Status> {
		crate::bootstrap::transaction_coordinator(&self.runtime)
			.abort(id)
			.await
	}
	async fn participants(&self) -> Result<Vec<LocalStatus>> {
		plumbing(&self.runtime, async |tx| AtomicParticipant::page(tx).await)
			.await
			.map(|rows| rows.into_iter().map(Into::into).collect())
			.map_err(Into::into)
	}
}
