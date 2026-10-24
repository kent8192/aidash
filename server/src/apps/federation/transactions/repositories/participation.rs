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

/// The Node of this Deployment that `manifest` names. A transaction names at
/// most one, and an inbound request was checked to be for exactly that Node.
fn local_node(runtime: &Federation, manifest: &Manifest) -> String {
	manifest
		.participants
		.iter()
		.map(|participant| participant.node_id.as_str())
		.find(|node| {
			aidash_domain::nodes::same_deployment(node, runtime.config.deployment.as_str())
		})
		.unwrap_or_default()
		.to_owned()
}

/// Fresh participant work shares the policy lease with Node deletion. Durable
/// finish/apply deliberately retains its original participant/decision authority.
pub(crate) async fn require_live_participant_in(
	tx: &mut dyn TransactionExecutor,
	runtime: &Federation,
	manifest: &Manifest,
) -> crate::Result<()> {
	let address = aidash_domain::nodes::NodeAddress::parse(&local_node(runtime, manifest))?;
	let tenant = crate::apps::identity::repositories::node_registrations::name_tenant(
		tx,
		address.node.as_str(),
	)
	.await?
	.ok_or(crate::Error::Forbidden)?;
	crate::apps::identity::models::AuthorizationBundle::lock_snapshot(tx, &tenant, false).await?;
	crate::apps::identity::repositories::nodes::require_live_locked(
		tx,
		&tenant,
		address.node.as_str(),
	)
	.await
}

pub(crate) async fn require_live_participant(
	runtime: &Federation,
	manifest: &Manifest,
) -> crate::Result<()> {
	runtime
		.store
		.control_pool
		.atomic(
			&aidash_application::tenancy::TenancyScope::Deployment(
				aidash_application::tenancy::DeploymentScopeReason::AtomicParticipant,
			),
			async |tx| require_live_participant_in(tx, runtime, manifest).await,
		)
		.await
}

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
		if matches!(next, ParticipantPhase::Reserved) {
			require_live_participant_in(self.transaction.executor(), &self.runtime, manifest)
				.await?;
		}
		let node = local_node(&self.runtime, manifest);
		let participant = manifest
			.local(&node)
			.map_err(|_| aidash_application::Error::Forbidden)?;
		let tx = self.transaction.executor();
		for mutation in &participant.mutations {
			let (kind, id) = match mutation {
				aidash_domain::transactions::Mutation::WorkspaceState { workspace_id, .. } => {
					("workspace", *workspace_id)
				}
				aidash_domain::transactions::Mutation::CompleteTask { task_id, .. } => {
					("task", *task_id)
				}
				aidash_domain::transactions::Mutation::FinishRun { run_id, .. } => ("run", *run_id),
			};
			super::mutation::require_target_node(tx, &node, kind, id).await?;
		}
		AtomicParticipant::insert(tx, manifest, phase(next))
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
		require_live_participant_in(self.transaction.executor(), &self.runtime, manifest).await?;
		let node = local_node(&self.runtime, manifest);
		let tx = self.transaction.executor();
		AtomicParticipant::mutation_context(tx, manifest.id).await?;
		tx.savepoint("participant_validation")
			.await
			.map_err(Error::from)?;
		aidash_application::transactions::mutation::apply(
			&mut crate::bootstrap::transaction_mutation_scope(tx, &node),
			&node,
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
		let node = local_node(&self.runtime, manifest);
		let tx = self.transaction.executor();
		AtomicParticipant::mutation_context(tx, manifest.id).await?;
		let mut scope = crate::bootstrap::transaction_mutation_scope(tx, &node);
		let applied =
			aidash_application::transactions::mutation::apply(&mut scope, &node, manifest).await;
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
	/// `admitted`'s receiving Node, as text.
	pub(crate) receiver: Option<String>,
}

#[async_trait]
impl ParticipantRepository for Repository {
	fn node_id(&self) -> &str {
		self.runtime.config.deployment.as_str()
	}
	fn receiver(&self) -> Option<&str> {
		self.receiver.as_deref()
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
