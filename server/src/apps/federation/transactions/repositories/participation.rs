//! Native scopes retain serializable validation, visibility gates and authority auditing.
use super::coordination::plumbing;
use crate::apps::{
	execution::repositories::inference,
	federation::transactions::{
		models::{AtomicGate, AtomicHistory, AtomicParticipant, states::AtomicParticipantPhase},
		serializers::contracts as native,
		services::{authority, fault},
	},
};
use crate::{Error, authorization::access::NativeAccess, federation::Federation};
use aidash_application::{
	Result,
	ports::transactions::participation::{
		ParticipantRepository, ParticipantScope, PendingParticipants,
	},
};
use aidash_domain::{
	provider::progress::ProgressOutcome,
	transactions::{
		Manifest,
		coordination::{LocalStatus, ParticipantPhase},
	},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder};
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
	/// An admitted rollback goes through the native transaction, whose lock epoch
	/// ends the catalog admissions that the released locks proved.
	async fn rollback_to_savepoint(
		&mut self,
		name: &str,
	) -> reinhardt::db::backends::error::Result<()> {
		match self {
			Self::Standalone(tx) => tx.rollback_to_savepoint(name).await,
			Self::Admitted(access) => access.tx.rollback_to_savepoint(name).await,
		}
	}
}

struct Scope {
	runtime: Federation,
	transaction: Transaction,
	/// Interruptions applied mutations wrote; recorded once the transaction commits.
	interruptions: Vec<ProgressOutcome>,
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
	async fn now(&mut self) -> Result<DateTime<Utc>> {
		// Deadline checks run after participant and gate locks may have waited, so they
		// read the wall clock rather than the transaction's fixed start time.
		let (sql, values) = Query::select()
			.expr_as(Expr::cust("CLOCK_TIMESTAMP()"), Alias::new("now"))
			.build(PostgresQueryBuilder);
		let row = self
			.transaction
			.executor()
			.fetch_one(&sql, convert_values(values))
			.await
			.map_err(Error::from)?;
		row.get("now")
			.map_err(FrameworkError::from)
			.map_err(Error::from)
			.map_err(Into::into)
	}
	async fn lock(&mut self, id: Uuid) -> Result<Option<LocalStatus>> {
		AtomicParticipant::lock(self.transaction.executor(), id)
			.await
			.map(|row| row.map(status))
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
			self.runtime.store.deployment.as_str(),
			manifest,
		)
		.await?;
		self.transaction
			.rollback_to_savepoint("participant_validation")
			.await
			.map_err(Error::from)?;
		self.transaction
			.executor()
			.release_savepoint("participant_validation")
			.await
			.map_err(Error::from)?;
		Ok(())
	}
	async fn apply_mutations(&mut self, manifest: &Manifest) -> Result<()> {
		let tx = self.transaction.executor();
		AtomicParticipant::mutation_context(tx, manifest.id).await?;
		let mut scope = crate::bootstrap::transaction_mutation_scope(tx);
		let applied = aidash_application::transactions::mutation::apply(
			&mut scope,
			self.runtime.store.deployment.as_str(),
			manifest,
		)
		.await;
		self.interruptions.extend(scope.1);
		applied
	}
	async fn finish(self: Box<Self>, result: Result<LocalStatus>) -> Result<LocalStatus> {
		let Scope {
			transaction,
			interruptions,
			..
		} = *self;
		let row = match transaction {
			Transaction::Admitted(access) => (*access).finish(result.map_err(Into::into)).await?,
			Transaction::Standalone(tx) => {
				let row = result?;
				tx.commit().await.map_err(Error::from)?;
				row
			}
		};
		inference::record_interruptions(&interruptions);
		Ok(row)
	}
	async fn rollback(self: Box<Self>) -> Result<()> {
		match self.transaction {
			Transaction::Standalone(tx) => {
				tx.rollback().await.map_err(Error::from).map_err(Into::into)
			}
			Transaction::Admitted(access) => access.rollback().await.map_err(Into::into),
		}
	}
}

struct Pending {
	rows: Vec<LocalStatus>,
}
impl PendingParticipants for Pending {
	fn rows(&self) -> &[LocalStatus] {
		&self.rows
	}
}

pub(crate) struct Repository {
	pub runtime: Federation,
	/// The Node that called, when this serves an inbound request.
	pub(crate) admitted: Option<aidash_application::federation::inbound::Admitted>,
}

#[async_trait]
impl ParticipantRepository for Repository {
	fn node_id(&self) -> &str {
		self.runtime.config.deployment.as_str()
	}
	async fn begin(&self) -> Result<Box<dyn ParticipantScope>> {
		let tx = AtomicParticipant::begin(&self.runtime.store.control_pool).await?;
		Ok(Box::new(Scope {
			runtime: self.runtime.clone(),
			transaction: Transaction::Standalone(tx),
			interruptions: Vec::new(),
		}))
	}
	async fn admission(
		&self,
		caller: &str,
		manifest: &Manifest,
	) -> Result<Option<Box<dyn ParticipantScope>>> {
		let Some(access) =
			authority::admission(&self.runtime, self.admitted.as_ref(), caller, manifest).await?
		else {
			return Ok(None);
		};
		Ok(Some(Box::new(Scope {
			runtime: self.runtime.clone(),
			transaction: Transaction::Admitted(Box::new(access.into_native()?)),
			interruptions: Vec::new(),
		})))
	}
	async fn pending(&self) -> Result<Box<dyn PendingParticipants>> {
		let rows = plumbing(&self.runtime, async |tx| {
			AtomicParticipant::pending(tx).await
		})
		.await?
		.into_iter()
		.map(status)
		.collect();
		Ok(Box::new(Pending { rows }))
	}
	async fn fault(&self, id: Uuid, point: &str) -> Result<()> {
		fault::cut(id, point).await.map_err(Into::into)
	}
	fn wake(&self) {
		self.runtime.notify.notify_waiters();
	}
}
