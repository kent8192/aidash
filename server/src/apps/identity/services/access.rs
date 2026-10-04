//! A live policy/credential lease used by typed execution boundaries.
use super::{
	Authorization, Snapshot,
	identity::SubjectIdentity,
	policy::{Decision, Evaluation, Resource},
};
use crate::apps::identity::models::AuthorizationDecision;
use crate::{Error, Result, store::Store};
use reinhardt::db::backends::{TransactionExecutor, dialect::postgres::PgTransactionExecutor};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use std::ops::{Deref, DerefMut};
use uuid::Uuid;

#[path = "visibility.rs"]
mod visibility;

pub(crate) struct AccessTransaction(Option<Transaction<'static, Postgres>>);

impl AccessTransaction {
	fn new(transaction: Transaction<'static, Postgres>) -> Self {
		Self(Some(transaction))
	}

	pub(in crate::apps::identity) fn is_active(&self) -> bool {
		self.0.is_some()
	}

	fn install(&mut self, transaction: Transaction<'static, Postgres>) {
		debug_assert!(self.0.is_none());
		self.0 = Some(transaction);
	}

	fn take(&mut self) -> Option<Transaction<'static, Postgres>> {
		self.0.take()
	}
}

impl Deref for AccessTransaction {
	type Target = Transaction<'static, Postgres>;

	fn deref(&self) -> &Self::Target {
		self.0
			.as_ref()
			.expect("authorization transaction is suspended")
	}
}

impl DerefMut for AccessTransaction {
	fn deref_mut(&mut self) -> &mut Self::Target {
		self.0
			.as_mut()
			.expect("authorization transaction is suspended")
	}
}

pub(crate) struct Access {
	pub marketplace_audit: Option<serde_json::Value>,
	pub core_gc_complete: bool,
	pub(super) remote_read_cache: std::collections::BTreeMap<(Uuid, String), bool>,
	pub(super) unavailable_peers: std::collections::BTreeSet<String>,
	pub(super) checking_reads: aidash_application::authorization::visits::ReadVisits,
	pub(crate) peer_client: reqwest::Client,
	pub(crate) node_id: String,
	pub(crate) dependency_frontier: Option<Vec<super::peer::dependencies::Reference>>,
	pub tx: AccessTransaction,
	pub identity: SubjectIdentity,
	pub snapshot: Snapshot,
	pub subjects: Vec<String>,
	pub durable_audit: bool,
	pub audit: bool,
	pub context: Value,
	pub inherited_lease: bool,
	pub approved_catalog: std::collections::BTreeSet<(String, String)>,
	pub cached_runs: std::collections::BTreeMap<(Uuid, Uuid), bool>,
	pub cached_humans: std::collections::BTreeMap<Uuid, bool>,
	pending_decisions: Vec<(Evaluation, Decision)>,
	pub(super) pool: PgPool,
	pub read_run: Option<Uuid>,
	pub read_grant: Option<Uuid>,
	pub(super) environment: Value,
}

impl Access {
	/// Native ports may update transport facts only after completing current authority checks.
	pub(crate) fn environment_mut(&mut self) -> &mut Value {
		&mut self.environment
	}

	/// Read membership uses a durable commit separate from the live authority lease.
	pub(crate) fn journal_pool(&self) -> &PgPool {
		&self.pool
	}
	// The same compound operation can narrow its subject chain or change the
	// workspace context. Never reuse a read decision from its earlier authority.
	pub(super) fn authority_context(&self) -> String {
		crate::registry::digest(
			&json!({"subjects":self.subjects,"context":self.context,"environment":self.environment}),
		)
	}
	pub async fn begin(store: &Store, identity: &SubjectIdentity) -> Result<Self> {
		Self::begin_with_lock(store, identity, false).await
	}

	pub async fn begin_exclusive(store: &Store, identity: &SubjectIdentity) -> Result<Self> {
		Self::begin_with_lock(store, identity, true).await
	}

	async fn begin_with_lock(
		store: &Store,
		identity: &SubjectIdentity,
		exclusive: bool,
	) -> Result<Self> {
		let mut tx = store.pool.begin().await?;
		let snapshot = identity.lock_with_mode(&mut tx, exclusive).await?;
		Ok(Self {
			marketplace_audit: None,
			core_gc_complete: false,
			remote_read_cache: Default::default(),
			unavailable_peers: Default::default(),
			checking_reads: Default::default(),
			dependency_frontier: None,
			peer_client: store.semantic_client.clone(),
			node_id: store.node_id.clone(),
			tx: AccessTransaction::new(tx),
			identity: identity.clone(),
			snapshot,
			subjects: vec![identity.subject.clone()],
			durable_audit: false,
			audit: true,
			context: json!({}),
			inherited_lease: false,
			approved_catalog: Default::default(),
			cached_runs: Default::default(),
			cached_humans: Default::default(),
			pending_decisions: vec![],
			pool: store.pool.clone(),
			read_run: None,
			read_grant: None,
			environment: json!({"node_id":store.node_id,"transport":"api"}),
		})
	}

	fn with_snapshot(
		store: &Store,
		identity: &SubjectIdentity,
		snapshot: Snapshot,
		tx: AccessTransaction,
	) -> Self {
		Self {
			dependency_frontier: None,
			marketplace_audit: None,
			core_gc_complete: false,
			remote_read_cache: Default::default(),
			unavailable_peers: Default::default(),
			checking_reads: Default::default(),
			peer_client: store.semantic_client.clone(),
			node_id: store.node_id.clone(),
			tx,
			identity: identity.clone(),
			snapshot,
			subjects: vec![identity.subject.clone()],
			durable_audit: false,
			audit: true,
			context: json!({}),
			inherited_lease: false,
			approved_catalog: Default::default(),
			cached_runs: Default::default(),
			cached_humans: Default::default(),
			pending_decisions: vec![],
			pool: store.pool.clone(),
			read_run: None,
			read_grant: None,
			environment: json!({"node_id":store.node_id,"transport":"api"}),
		}
	}

	// The caller retains the outer Access until this mutation commits. Reusing
	// its locks avoids queuing a second shared lock behind a waiting revoker.
	pub async fn under_lease(lease: &Self) -> Result<Self> {
		let tx = lease.pool.begin().await?;
		Ok(Self {
			marketplace_audit: None,
			core_gc_complete: false,
			remote_read_cache: Default::default(),
			unavailable_peers: Default::default(),
			checking_reads: Default::default(),
			dependency_frontier: None,
			peer_client: lease.peer_client.clone(),
			node_id: lease.node_id.clone(),
			tx: AccessTransaction::new(tx),
			identity: lease.identity.clone(),
			snapshot: lease.snapshot.clone(),
			subjects: lease.subjects.clone(),
			durable_audit: false,
			audit: true,
			context: lease.context.clone(),
			inherited_lease: true,
			approved_catalog: lease.approved_catalog.clone(),
			cached_runs: Default::default(),
			cached_humans: Default::default(),
			pending_decisions: vec![],
			pool: lease.pool.clone(),
			read_run: lease.read_run,
			read_grant: lease.read_grant,
			environment: lease.environment.clone(),
		})
	}

	/// Commit the completed authorization checks and release their row locks
	/// while the worker waits on an external provider.
	pub async fn suspend(&mut self) -> Result<()> {
		for (input, decision) in &self.pending_decisions {
			Authorization::record(&mut self.tx, &self.identity.tenant, input, decision).await?;
		}
		let previous = self
			.tx
			.take()
			.ok_or_else(|| Error::Conflict("authorization transaction is suspended".into()))?;
		previous.commit().await?;
		self.pending_decisions.clear();
		Ok(())
	}

	/// Start a fresh execution authorization boundary after an external wait.
	pub async fn refresh_execution(&mut self, run_id: Uuid) -> Result<()> {
		if !self.tx.is_active() {
			self.tx.install(self.pool.begin().await?);
		}
		self.snapshot = self.identity.lock_with_mode(&mut self.tx, false).await?;
		self.remote_read_cache.clear();
		self.unavailable_peers.clear();
		self.checking_reads.clear();
		self.dependency_frontier = None;
		self.subjects = vec![self.identity.subject.clone()];
		self.durable_audit = true;
		self.audit = true;
		self.context = json!({});
		self.inherited_lease = false;
		self.approved_catalog.clear();
		self.cached_runs.clear();
		self.cached_humans.clear();
		self.pending_decisions.clear();
		self.read_run = Some(run_id);
		self.read_grant = None;
		self.worker();
		Ok(())
	}

	pub(in crate::apps::identity) async fn discard_failed_execution_refresh(&mut self) {
		if let Some(transaction) = self.tx.take()
			&& let Err(error) = transaction.rollback().await
		{
			tracing::debug!(%error, "failed execution-refresh transaction rollback");
		}
		self.pending_decisions.clear();
	}

	pub fn resource(&self, kind: &str, id: impl ToString, attributes: Value) -> Resource {
		aidash_application::authorization::lease::resource(
			&self.identity.tenant,
			&self.context,
			kind,
			&id.to_string(),
			attributes,
		)
	}

	pub fn worker(&mut self) {
		self.environment["transport"] = json!("worker");
	}

	pub async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		aidash_application::authorization::workspaces::resource(
			&mut crate::bootstrap::run_visibility_scope(self),
			id,
		)
		.await
		.map_err(Into::into)
	}

	pub async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		aidash_application::authorization::lease::decide(
			&mut crate::bootstrap::run_visibility_scope(self),
			resource,
			action,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) fn evaluation(
		&self,
		subject: &str,
		resource: &Resource,
		action: &str,
	) -> Evaluation {
		Evaluation {
			subject: subject.into(),
			action: action.into(),
			resource: resource.clone(),
			environment: self.environment.clone(),
		}
	}

	pub(crate) async fn record(&mut self, records: &[(Evaluation, Decision)]) -> Result<()> {
		if !self.audit {
			return Ok(());
		}
		// A worker step spans the existing invocation-start/effect/result
		// commits. Keep authority locked for that entire boundary, and commit
		// its decision audit before effects so a killed worker cannot lose it.
		if self.durable_audit {
			let mut audit: Box<dyn TransactionExecutor> =
				Box::new(PgTransactionExecutor::new(self.pool.begin().await?));
			for (input, decision) in records {
				AuthorizationDecision::append(
					audit.as_mut(),
					&self.identity.tenant,
					input,
					decision,
				)
				.await?;
			}
			audit.commit().await?;
		} else {
			self.pending_decisions.extend_from_slice(records);
		}
		Ok(())
	}

	pub async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		if self.decide(resource, action).await? {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}

	pub async fn finish<T>(mut self, result: Result<T>) -> Result<T> {
		let tx = self
			.tx
			.take()
			.map(|tx| Box::new(PgTransactionExecutor::new(tx)) as Box<dyn TransactionExecutor>);
		self.complete(tx, result).await
	}

	/// Keep the policy and credential locks while transferring the same physical
	/// transaction to Reinhardt for the protected mutation and its audit.
	pub fn into_native(mut self) -> Result<NativeAccess> {
		let transaction = self
			.tx
			.take()
			.ok_or_else(|| Error::Conflict("authorization transaction is suspended".into()))?;
		Ok(NativeAccess {
			tx: Box::new(PgTransactionExecutor::new(transaction)),
			access: self,
		})
	}

	async fn complete<T>(
		self,
		tx: Option<Box<dyn TransactionExecutor>>,
		result: Result<T>,
	) -> Result<T> {
		if result.is_ok() {
			let mut tx =
				tx.ok_or_else(|| Error::Conflict("authorization transaction is suspended".into()))?;
			for (input, decision) in &self.pending_decisions {
				AuthorizationDecision::append(tx.as_mut(), &self.identity.tenant, input, decision)
					.await?;
			}
			tx.commit().await?;
		} else {
			if let Some(tx) = tx {
				tx.rollback().await?;
			}
			if matches!(result, Err(Error::Forbidden)) {
				// Denials survive rollback with the evaluated policy revision.
				let mut audit: Box<dyn TransactionExecutor> =
					Box::new(PgTransactionExecutor::new(self.pool.begin().await?));
				for (input, decision) in &self.pending_decisions {
					AuthorizationDecision::append(
						audit.as_mut(),
						&self.identity.tenant,
						input,
						decision,
					)
					.await?;
				}
				audit.commit().await?;
			}
		}
		result
	}
}

pub(crate) struct NativeAccess {
	pub tx: Box<dyn TransactionExecutor>,
	access: Access,
}

impl NativeAccess {
	pub(crate) async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		if self.decide(resource, action).await? {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}

	pub fn set_context(&mut self, context: Value) {
		self.access.context = context;
	}
	pub async fn begin(store: &Store, identity: &SubjectIdentity) -> Result<Self> {
		let mut tx: Box<dyn TransactionExecutor> =
			Box::new(PgTransactionExecutor::new(store.pool.begin().await?));
		let snapshot = identity.lock_native(tx.as_mut(), false).await?;
		Ok(Self {
			tx,
			access: Access::with_snapshot(store, identity, snapshot, AccessTransaction(None)),
		})
	}

	pub fn resource(&self, kind: &str, id: impl ToString, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}

	pub async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access.decide(resource, action).await
	}

	pub async fn finish<T>(self, result: Result<T>) -> Result<T> {
		self.access.complete(Some(self.tx), result).await
	}
}

impl Access {
	pub(crate) fn output_visit(
		&self,
		grant: Uuid,
	) -> Option<aidash_application::authorization::visits::ReadVisit> {
		self.checking_reads.enter((
			self.node_id.clone(),
			grant,
			format!("remote:{}", self.authority_context()),
		))
	}
}

impl NativeAccess {
	pub(crate) fn tenant_transaction(&mut self) -> (&mut dyn TransactionExecutor, &str) {
		(self.tx.as_mut(), &self.access.identity.tenant)
	}
	pub(crate) fn catalog_approved(&self, reference: &crate::registry::EntityRef) -> bool {
		self.access
			.approved_catalog
			.contains(&(reference.id.clone(), reference.version.clone()))
	}
	pub(crate) fn remember_catalog(&mut self, reference: &crate::registry::EntityRef) {
		self.access
			.approved_catalog
			.insert((reference.id.clone(), reference.version.clone()));
	}
}

impl NativeAccess {
	pub(crate) fn remote_identity(&self) -> (&str, &str) {
		(&self.access.identity.tenant, &self.access.identity.subject)
	}
	pub(crate) fn remote_transport_parts(&self) -> (&reqwest::Client, &str) {
		(&self.access.peer_client, &self.access.node_id)
	}
	pub(crate) fn cached_remote_read(&self, run: Uuid) -> Option<bool> {
		self.access.cached_remote_read(run)
	}
	pub(crate) fn remember_remote_read(&mut self, run: Uuid, visible: bool) {
		self.access.remember_remote_read(run, visible);
	}
	pub(crate) fn peer_unavailable(&self, node: &str) -> bool {
		self.access.peer_unavailable(node)
	}
	pub(crate) fn mark_peer_unavailable(&mut self, node: &str) {
		self.access.mark_peer_unavailable(node);
	}
}

impl Access {
	pub(crate) fn cached_remote_read(&self, run: Uuid) -> Option<bool> {
		self.remote_read_cache
			.get(&(run, self.authority_context()))
			.copied()
	}
	pub(crate) fn remember_remote_read(&mut self, run: Uuid, visible: bool) {
		self.remote_read_cache
			.insert((run, self.authority_context()), visible);
	}
	pub(crate) fn peer_unavailable(&self, node: &str) -> bool {
		self.unavailable_peers.contains(node)
	}
	pub(crate) fn mark_peer_unavailable(&mut self, node: &str) {
		self.unavailable_peers.insert(node.into());
	}
}

impl Access {
	pub(crate) fn environment(&self) -> &Value {
		&self.environment
	}
}
impl NativeAccess {
	pub(crate) fn workspace_context(&self) -> &Value {
		&self.access.context
	}
}

impl Access {
	/// Read the same current policy environment used by worker inference decisions.
	pub(in crate::apps::identity) fn execution_node(&self) -> Option<&str> {
		self.environment["node_id"].as_str()
	}
}

impl Access {
	/// Owned recursion leases retain the current native credential and policy context key.
	pub(crate) fn authority_read_visit(
		&self,
		kind: &str,
		id: Uuid,
	) -> Option<aidash_application::authorization::visits::ReadVisit> {
		self.checking_reads.enter((
			self.node_id.clone(),
			id,
			format!("{kind}:{}", self.authority_context()),
		))
	}
}
