//! Native records and authority adapters for transaction management use cases.
use super::{authority::persistence, coordination::connection};
use crate::apps::federation::transactions::{
	models::{AtomicParticipant, AtomicPeerTrust, coordinator_records},
	serializers::protocol::TransactionTrust,
};
use crate::{Error, federation::Federation};
use aidash_application::{
	Result,
	ports::transactions::management::{ManagementRepository, TrustScope},
	transactions::{admission, authority::control},
};
use aidash_domain::{
	federation::Peer,
	identity::execution::ExecutionPrincipal,
	transactions::{
		Manifest,
		authority::{Origin, Status},
		coordination::{LocalStatus, Vote},
		management::{History, Trust},
	},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::db::orm::DatabaseConnectionLease;
use uuid::Uuid;

pub(crate) struct Repository {
	pub runtime: Federation,
}

struct TrustUpdate {
	runtime: Federation,
	connection: DatabaseConnectionLease,
}
#[async_trait]
impl TrustScope for TrustUpdate {
	async fn set(&mut self, trust: Trust) -> Result<Trust> {
		AtomicPeerTrust::set(self.connection.handle(), TransactionTrust::from(trust))
			.await
			.map(Into::into)
			.map_err(Into::into)
	}
	async fn pending_peer(&mut self, node: &str) -> Result<Vec<Uuid>> {
		persistence::pending_peer(&self.runtime, node)
			.await
			.map_err(Into::into)
	}
}

#[async_trait]
impl ManagementRepository for Repository {
	async fn submit_operator(&self, manifest: &Manifest) -> Result<Status> {
		admission::submit(
			&crate::bootstrap::transaction_admission_repository(&self.runtime),
			&crate::bootstrap::registry_validation_for(&self.runtime.store),
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
		control::submit(
			&crate::bootstrap::transaction_authority_repository(&self.runtime),
			identity,
			manifest,
		)
		.await
	}
	async fn candidates(
		&self,
		identity: Option<&ExecutionPrincipal>,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<Status>> {
		let lease = connection(&self.runtime)?;
		coordinator_records::visible_candidates(&mut lease.handle(), identity, cursor)
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
		let lease = connection(&self.runtime)?;
		let mut db = lease.handle();
		let votes = coordinator_records::votes(&mut db, id)
			.await?
			.into_iter()
			.map(Into::into)
			.collect();
		let history = coordinator_records::audit(&mut db, id)
			.await?
			.into_iter()
			.map(Into::into)
			.collect();
		Ok((votes, history))
	}
	async fn abort_operator(&self, id: Uuid) -> Result<Status> {
		crate::bootstrap::transaction_coordinator(&self.runtime)
			.abort(id)
			.await
	}
	async fn participants(&self) -> Result<Vec<LocalStatus>> {
		let lease = connection(&self.runtime)?;
		AtomicParticipant::page(&mut lease.handle())
			.await
			.map(|rows| rows.into_iter().map(Into::into).collect())
			.map_err(Into::into)
	}
	async fn trusts(&self) -> Result<Vec<Trust>> {
		let lease = connection(&self.runtime)?;
		AtomicPeerTrust::page(&mut lease.handle())
			.await
			.map(|rows| rows.into_iter().map(Into::into).collect())
			.map_err(Into::into)
	}
	async fn ensure_peer(&self, node: &str) -> Result<()> {
		self.runtime
			.peer(node)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn trust_scope(&self) -> Result<Box<dyn TrustScope>> {
		Ok(Box::new(TrustUpdate {
			runtime: self.runtime.clone(),
			connection: connection(&self.runtime)?,
		}))
	}
	async fn pending_peer(&self, node: &str) -> Result<Vec<Uuid>> {
		persistence::pending_peer(&self.runtime, node)
			.await
			.map_err(Into::into)
	}
	fn credential(&self, reference: &str) -> Result<String> {
		crate::config::peer_secret(reference).map_err(Into::into)
	}
	async fn restore_peer(&self, node: &str, reference: &str, credential: &str) -> Result<Peer> {
		let lease = connection(&self.runtime)?;
		crate::apps::federation::peer::models::records::restore_authentication(
			lease.handle(),
			node,
			reference,
			credential,
		)
		.await
		.map_err(Into::into)
	}
}
