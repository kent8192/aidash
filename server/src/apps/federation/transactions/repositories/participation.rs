//! Native scopes retain serializable validation, visibility gates and authority auditing.
use super::coordination::connection;
use crate::apps::federation::transactions::{
	models::{
		AtomicGate, AtomicHistory, AtomicParticipant, AtomicPeerTrust,
		states::AtomicParticipantPhase,
	},
	serializers::contracts as native,
	services::{authority, fault},
};
use crate::{Error, authorization::access::NativeAccess, federation::Federation};
use aidash_application::{
	Result,
	ports::transactions::participation::{
		ParticipantRepository, ParticipantScope, PendingParticipants,
	},
};
use aidash_domain::transactions::{
	Manifest,
	coordination::{LocalStatus, ParticipantPhase},
};
use async_trait::async_trait;
use reinhardt::db::{
	backends::{DatabaseConnection, PostgresBackend, TransactionExecutor},
	orm::DatabaseConnectionLease,
};
use std::sync::Arc;
use uuid::Uuid;

enum Transaction {
	Standalone(Box<dyn TransactionExecutor>),
	Admitted(Box<NativeAccess>),
}

impl Transaction {
	fn executor(&mut self) -> &mut dyn TransactionExecutor {
		match self {
			Self::Standalone(tx) => tx.as_mut(),
			Self::Admitted(access) => access.tx.as_mut(),
		}
	}
}

struct Scope {
	runtime: Federation,
	transaction: Transaction,
}

fn phase(phase: ParticipantPhase) -> AtomicParticipantPhase {
	match phase {
		ParticipantPhase::Reserved => AtomicParticipantPhase::Reserved,
		ParticipantPhase::Prepared => AtomicParticipantPhase::Prepared,
		ParticipantPhase::Applied => AtomicParticipantPhase::Applied,
		ParticipantPhase::Committed => AtomicParticipantPhase::Committed,
		ParticipantPhase::Aborted => AtomicParticipantPhase::Aborted,
	}
}

fn status(row: AtomicParticipant) -> LocalStatus {
	native::LocalStatus::from(row).into()
}

#[async_trait]
impl ParticipantScope for Scope {
	async fn lock(&mut self, id: Uuid) -> Result<Option<LocalStatus>> {
		AtomicParticipant::lock(self.transaction.executor(), id)
			.await
			.map(|row| row.map(status))
			.map_err(Into::into)
	}
	async fn trusted(&mut self, node: &str) -> Result<bool> {
		AtomicPeerTrust::permits(self.transaction.executor(), node)
			.await
			.map_err(Into::into)
	}
	async fn gate_exclusive(&mut self) -> Result<Option<Uuid>> {
		AtomicGate::lock_exclusive(self.transaction.executor())
			.await
			.map_err(Into::into)
	}
	async fn reserve_gate(&mut self, id: Uuid) -> Result<()> {
		AtomicGate::reserve(self.transaction.executor(), id)
			.await
			.map_err(Into::into)
	}
	async fn release_gate(&mut self, committed: bool) -> Result<()> {
		AtomicGate::release(self.transaction.executor(), committed)
			.await
			.map_err(Into::into)
	}
	async fn insert(&mut self, manifest: &Manifest, next: ParticipantPhase) -> Result<LocalStatus> {
		AtomicParticipant::insert(self.transaction.executor(), manifest, phase(next))
			.await
			.map(status)
			.map_err(Into::into)
	}
	async fn transition(&mut self, id: Uuid, next: ParticipantPhase) -> Result<LocalStatus> {
		AtomicParticipant::transition(self.transaction.executor(), id, phase(next))
			.await
			.map(status)
			.map_err(Into::into)
	}
	async fn reservation_history(&mut self, id: Uuid) -> Result<()> {
		AtomicHistory::append(
			self.transaction.executor(),
			id,
			"participant",
			"RESERVED",
			"node visibility barrier persisted",
		)
		.await
		.map_err(Into::into)
	}
	async fn validate_mutations(&mut self, manifest: &Manifest) -> Result<()> {
		let tx = self.transaction.executor();
		AtomicParticipant::mutation_context(tx, manifest.id).await?;
		tx.savepoint("participant_validation")
			.await
			.map_err(Error::from)?;
		aidash_application::transactions::mutation::apply(
			&mut crate::bootstrap::transaction_mutation_scope(tx),
			&crate::bootstrap::registry_validation(),
			&self.runtime.store.node_id,
			manifest,
		)
		.await?;
		tx.rollback_to_savepoint("participant_validation")
			.await
			.map_err(Error::from)?;
		tx.release_savepoint("participant_validation")
			.await
			.map_err(Error::from)?;
		Ok(())
	}
	async fn apply_mutations(&mut self, manifest: &Manifest) -> Result<()> {
		let tx = self.transaction.executor();
		AtomicParticipant::mutation_context(tx, manifest.id).await?;
		aidash_application::transactions::mutation::apply(
			&mut crate::bootstrap::transaction_mutation_scope(tx),
			&crate::bootstrap::registry_validation(),
			&self.runtime.store.node_id,
			manifest,
		)
		.await
	}
	async fn finish(self: Box<Self>, result: Result<LocalStatus>) -> Result<LocalStatus> {
		match self.transaction {
			Transaction::Admitted(access) => (*access)
				.finish(result.map_err(Into::into))
				.await
				.map_err(Into::into),
			Transaction::Standalone(tx) => {
				let row = result?;
				tx.commit().await.map_err(Error::from)?;
				Ok(row)
			}
		}
	}
	async fn rollback(self: Box<Self>) -> Result<()> {
		let tx = match self.transaction {
			Transaction::Standalone(tx) => tx,
			Transaction::Admitted(access) => (*access).tx,
		};
		tx.rollback().await.map_err(Error::from).map_err(Into::into)
	}
}

struct Pending {
	rows: Vec<LocalStatus>,
	_connection: DatabaseConnectionLease,
}
impl PendingParticipants for Pending {
	fn rows(&self) -> &[LocalStatus] {
		&self.rows
	}
}

pub(crate) struct Repository {
	pub runtime: Federation,
}

#[async_trait]
impl ParticipantRepository for Repository {
	fn node_id(&self) -> &str {
		&self.runtime.config.node_id
	}
	async fn begin(&self) -> Result<Box<dyn ParticipantScope>> {
		let db = DatabaseConnection::new(Arc::new(PostgresBackend::new(
			self.runtime.store.control_pool.clone(),
		)));
		let tx = AtomicParticipant::begin(&db).await?;
		Ok(Box::new(Scope {
			runtime: self.runtime.clone(),
			transaction: Transaction::Standalone(tx),
		}))
	}
	async fn admission(
		&self,
		caller: &str,
		manifest: &Manifest,
	) -> Result<Option<Box<dyn ParticipantScope>>> {
		let Some(access) = authority::admission(&self.runtime, caller, manifest).await? else {
			return Ok(None);
		};
		Ok(Some(Box::new(Scope {
			runtime: self.runtime.clone(),
			transaction: Transaction::Admitted(Box::new(access.into_native()?)),
		})))
	}
	async fn pending(&self) -> Result<Box<dyn PendingParticipants>> {
		let lease = connection(&self.runtime)?;
		let rows = AtomicParticipant::pending(&mut lease.handle())
			.await?
			.into_iter()
			.map(status)
			.collect();
		Ok(Box::new(Pending {
			rows,
			_connection: lease,
		}))
	}
	async fn fault(&self, id: Uuid, point: &str) -> Result<()> {
		fault::cut(id, point).await.map_err(Into::into)
	}
	fn wake(&self) {
		self.runtime.notify.notify_waiters();
	}
}
