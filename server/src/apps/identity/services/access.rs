//! A live policy/credential lease used by typed execution boundaries.
use super::{
	Authorization, Snapshot,
	identity::SubjectIdentity,
	policy::{Decision, Evaluation, Resource},
};
use crate::apps::identity::models::AuthorizationDecision;
use crate::database::native::Pool;
use crate::{Error, Result, store::Store};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::{Value, json};
use std::ops::{Deref, DerefMut};
use uuid::Uuid;

#[path = "visibility.rs"]
mod visibility;

/// Every input of one catalog admission decision: who decides, under which policy
/// revision and inherited fence, with which evaluation context and audit mode, for
/// which definition. A decision first made unaudited or with deferred audit is never
/// reused where it must be recorded durably.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CatalogAdmission {
	tenant: String,
	node: Option<aidash_domain::nodes::NodeName>,
	subject: String,
	revision: i64,
	inherited_lease: bool,
	audit: bool,
	durable_audit: bool,
	authority: String,
	id: String,
	version: String,
	action: String,
}

#[derive(Default)]
pub(crate) struct AccessTransaction {
	transaction: Option<crate::database::native::Transaction>,
	/// Admissions proven on this physical transaction. Their distribution, catalog and
	/// policy locks last exactly as long, so the record ends with the transaction or
	/// with a savepoint rollback that may have released them (its lock epoch).
	admissions: std::collections::BTreeMap<CatalogAdmission, crate::registry::Entry>,
	admissions_epoch: u64,
}

impl AccessTransaction {
	fn new(transaction: crate::database::native::Transaction) -> Self {
		Self {
			transaction: Some(transaction),
			admissions: Default::default(),
			admissions_epoch: 0,
		}
	}

	pub(crate) fn is_active(&self) -> bool {
		self.transaction.is_some()
	}

	fn install(&mut self, transaction: crate::database::native::Transaction) {
		debug_assert!(self.transaction.is_none());
		self.admissions.clear();
		self.transaction = Some(transaction);
	}

	fn take(&mut self) -> Option<crate::database::native::Transaction> {
		self.admissions.clear();
		self.transaction.take()
	}

	/// The executor for checks that only read rows for Node-backed subjects.
	pub(crate) fn executor(&mut self) -> Option<&mut dyn TransactionExecutor> {
		self.transaction
			.as_mut()
			.map(|transaction| transaction as &mut dyn TransactionExecutor)
	}

	pub(crate) fn admitted(&self, admission: &CatalogAdmission) -> Option<crate::registry::Entry> {
		let transaction = self.transaction.as_ref()?;
		if transaction.lock_epoch() != self.admissions_epoch {
			return None;
		}
		self.admissions.get(admission).cloned()
	}

	pub(crate) fn admit(&mut self, admission: CatalogAdmission, entry: crate::registry::Entry) {
		let Some(transaction) = &self.transaction else {
			return;
		};
		let epoch = transaction.lock_epoch();
		if epoch != self.admissions_epoch {
			self.admissions.clear();
			self.admissions_epoch = epoch;
		}
		self.admissions.insert(admission, entry);
	}
}

impl Deref for AccessTransaction {
	type Target = crate::database::native::Transaction;

	fn deref(&self) -> &Self::Target {
		self.transaction
			.as_ref()
			.expect("authorization transaction is suspended")
	}
}

impl DerefMut for AccessTransaction {
	fn deref_mut(&mut self) -> &mut Self::Target {
		self.transaction
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
	pub(crate) read_verdicts: aidash_application::authorization::visits::ReadVerdicts,
	/// How this authority's reads reach other Nodes. A transaction lent to a
	/// disclosure check holds none, so its remote proofs are unavailable.
	pub(crate) wire: Option<crate::apps::federation::remote::services::wire::Wire>,
	pub(crate) node_id: String,
	/// The Node of the Workspace or Run this authority serves, once known.
	/// Catalog Approvals are per Node; an Access with no Node yet uses the
	/// Tenant's default Node.
	pub(crate) node: Option<aidash_domain::nodes::NodeName>,
	/// The address of `node`, which Federation references name Nodes by.
	pub(crate) node_address: Option<String>,
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
	pub(super) pool: Pool,
	pub read_run: Option<Uuid>,
	pub read_grant: Option<Uuid>,
	/// A batchable read loads its Working Area and records without excluding concurrent reads.
	pub shared_area: bool,
	/// A remote worker's Home Workspace corpus digest, from the describe reply
	/// that refreshed this authority (#191). `None` for every other Access.
	pub(crate) home_corpus_digest: Option<String>,
	pub(super) environment: Value,
}

/// All identity-sensitive caches are isolated across selected-disclosure checks.
pub(crate) struct PublicationContext {
	identity: SubjectIdentity,
	subjects: Vec<String>,
	context: Value,
	environment: Value,
	read_run: Option<Uuid>,
	read_grant: Option<Uuid>,
	approved_catalog: std::collections::BTreeSet<(String, String)>,
	cached_runs: std::collections::BTreeMap<(Uuid, Uuid), bool>,
	cached_humans: std::collections::BTreeMap<Uuid, bool>,
	remote_read_cache: std::collections::BTreeMap<(Uuid, String), bool>,
	checking_reads: aidash_application::authorization::visits::ReadVisits,
	read_verdicts: aidash_application::authorization::visits::ReadVerdicts,
	dependency_frontier: Option<Vec<super::peer::dependencies::Reference>>,
}

/// Drop restores identity even when a caller cancels a publication check.
pub(crate) struct PublicationView<'a> {
	access: &'a mut Access,
	old: Option<PublicationContext>,
}
impl Deref for PublicationView<'_> {
	type Target = Access;
	fn deref(&self) -> &Access {
		self.access
	}
}
impl DerefMut for PublicationView<'_> {
	fn deref_mut(&mut self) -> &mut Access {
		self.access
	}
}
impl Drop for PublicationView<'_> {
	fn drop(&mut self) {
		if let Some(old) = self.old.take() {
			self.access.leave_publisher(old);
		}
	}
}

impl Access {
	/// The scope of every transaction this authority opens: its subject's Tenant.
	pub(crate) fn tenancy(&self) -> Result<aidash_application::tenancy::TenancyScope> {
		self.identity.tenancy()
	}

	/// Native ports may update transport facts only after completing current authority checks.
	pub(crate) fn environment_mut(&mut self) -> &mut Value {
		&mut self.environment
	}

	/// Read membership uses a durable commit separate from the live authority lease.
	pub(crate) fn journal_pool(&self) -> &Pool {
		&self.pool
	}
	// The same compound operation can narrow its subject chain or change the
	// workspace context. Never reuse a read decision from its earlier authority.
	pub(super) fn authority_context(&self) -> String {
		crate::registry::digest(
			&json!({"subjects":self.subjects,"context":self.context,"environment":self.environment}),
		)
	}
	pub(crate) fn catalog_admission(
		&self,
		reference: &crate::registry::EntityRef,
		action: &str,
	) -> CatalogAdmission {
		CatalogAdmission {
			tenant: self.identity.tenant.clone(),
			node: self.node.clone(),
			subject: self.identity.subject.clone(),
			revision: self.snapshot.revision,
			inherited_lease: self.inherited_lease,
			audit: self.audit,
			durable_audit: self.durable_audit,
			authority: self.authority_context(),
			id: reference.id.clone(),
			version: reference.version.clone(),
			action: action.into(),
		}
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
		Self::begin_on(
			&store.pool,
			store.deployment.as_str(),
			Some(crate::apps::federation::remote::services::wire::Wire::of_store(store)),
			identity,
			exclusive,
		)
		.await
	}

	/// The authority of a subject of the Tenant's own policy that holds no
	/// credential: a Node of the same Tenant acting for one of its subjects
	/// (ADR 0064). Its policy snapshot is locked like any authority's; code
	/// that needs a credential, such as a durable Run identity, fails closed on
	/// the nil credential.
	pub(crate) async fn begin_subject(
		store: &Store,
		tenant: &str,
		subject: &str,
		exclusive: bool,
	) -> Result<Self> {
		let identity = SubjectIdentity {
			http_session: None,
			credential_id: Uuid::nil(),
			tenant: tenant.into(),
			subject: subject.into(),
		};
		let mut tx = store.pool.begin(&identity.tenancy()?).await?;
		let snapshot = Authorization::load_with_mode(&mut tx, tenant, exclusive).await?;
		super::identity::require_live_subject(&mut *tx, &snapshot, subject).await?;
		Ok(Self::from_transaction(
			tx,
			&store.pool,
			store.deployment.as_str(),
			Some(crate::apps::federation::remote::services::wire::Wire::of_store(store)),
			&identity,
			snapshot,
		))
	}

	/// Revalidate another saved disclosure authority using existing pool capacity.
	pub(crate) async fn begin_on(
		pool: &Pool,
		node: &str,
		wire: Option<crate::apps::federation::remote::services::wire::Wire>,
		identity: &SubjectIdentity,
		exclusive: bool,
	) -> Result<Self> {
		identity.check_binding(pool).await?;
		let mut tx = pool.begin(&identity.tenancy()?).await?;
		let snapshot = identity.lock_with_mode(&mut tx, exclusive).await?;
		Ok(Self::from_transaction(
			tx, pool, node, wire, identity, snapshot,
		))
	}

	pub(crate) fn from_transaction(
		tx: crate::database::native::Transaction,
		pool: &Pool,
		node: &str,
		wire: Option<crate::apps::federation::remote::services::wire::Wire>,
		identity: &SubjectIdentity,
		snapshot: Snapshot,
	) -> Self {
		debug_assert!(
			tx.scope()
				.is_none_or(|scope| scope.tenant_name().map(|name| name.as_str())
					== Some(identity.tenant.as_str())),
			"an authority transaction is opened in its subject's Tenant scope"
		);
		Self {
			marketplace_audit: None,
			core_gc_complete: false,
			remote_read_cache: Default::default(),
			unavailable_peers: Default::default(),
			checking_reads: Default::default(),
			read_verdicts: Default::default(),
			dependency_frontier: None,
			wire,
			node_id: node.to_owned(),
			node: None,
			node_address: None,
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
			pool: pool.clone(),
			read_run: None,
			read_grant: None,
			shared_area: false,
			home_corpus_digest: None,
			environment: json!({"node_id":node,"transport":"api"}),
		}
	}
	pub(crate) async fn flush_audit(&mut self) -> Result<()> {
		Authorization::record_many(&mut self.tx, &self.identity.tenant, &self.pending_decisions)
			.await?;
		self.pending_decisions.clear();
		Ok(())
	}
	pub(crate) fn return_transaction(&mut self) -> crate::database::native::Transaction {
		self.tx.take().expect("active publication transaction")
	}
	/// Scope publisher checks on this physical transaction, preserving the reader's context.
	pub(crate) async fn enter_publisher(
		&mut self,
		identity: SubjectIdentity,
		subjects: Vec<String>,
	) -> Result<PublicationContext> {
		if identity.tenant != self.identity.tenant
			|| !subjects.contains(&identity.subject)
			|| subjects
				.iter()
				.any(|s| !super::identity::enabled(&self.snapshot, s))
		{
			return Err(Error::Forbidden);
		}
		for subject in &subjects {
			super::identity::require_live_subject_on(self.tx.executor(), &self.snapshot, subject)
				.await?;
		}
		if identity.credential_id != self.identity.credential_id {
			identity.lock_credential(&mut self.tx).await?;
		}
		let old = PublicationContext {
			identity: std::mem::replace(&mut self.identity, identity),
			subjects: std::mem::replace(&mut self.subjects, subjects),
			context: std::mem::replace(&mut self.context, json!({})),
			environment: std::mem::replace(
				&mut self.environment,
				json!({"node_id":self.node_id,"transport":"worker"}),
			),
			read_run: self.read_run.take(),
			read_grant: self.read_grant.take(),
			approved_catalog: std::mem::take(&mut self.approved_catalog),
			cached_runs: std::mem::take(&mut self.cached_runs),
			cached_humans: std::mem::take(&mut self.cached_humans),
			remote_read_cache: std::mem::take(&mut self.remote_read_cache),
			checking_reads: std::mem::take(&mut self.checking_reads),
			read_verdicts: std::mem::take(&mut self.read_verdicts),
			dependency_frontier: self.dependency_frontier.take(),
		};
		self.worker();
		Ok(old)
	}
	pub(crate) async fn publisher_view(
		&mut self,
		identity: SubjectIdentity,
		subjects: Vec<String>,
	) -> Result<PublicationView<'_>> {
		let old = self.enter_publisher(identity, subjects).await?;
		Ok(PublicationView {
			access: self,
			old: Some(old),
		})
	}

	pub(crate) fn leave_publisher(&mut self, old: PublicationContext) {
		self.identity = old.identity;
		self.subjects = old.subjects;
		self.context = old.context;
		self.environment = old.environment;
		self.read_run = old.read_run;
		self.read_grant = old.read_grant;
		self.approved_catalog = old.approved_catalog;
		self.cached_runs = old.cached_runs;
		self.cached_humans = old.cached_humans;
		self.remote_read_cache = old.remote_read_cache;
		self.checking_reads = old.checking_reads;
		self.read_verdicts = old.read_verdicts;
		self.dependency_frontier = old.dependency_frontier;
	}

	// The caller retains the outer Access until this mutation commits. Reusing
	// its locks avoids queuing a second shared lock behind a waiting revoker.
	pub async fn under_lease(lease: &Self) -> Result<Self> {
		Self::under_lease_on(lease, &lease.pool).await
	}
	pub(crate) async fn under_lease_on(lease: &Self, pool: &Pool) -> Result<Self> {
		let tx = pool.begin(&lease.identity.tenancy()?).await?;
		Ok(Self {
			marketplace_audit: None,
			core_gc_complete: false,
			remote_read_cache: Default::default(),
			unavailable_peers: Default::default(),
			checking_reads: Default::default(),
			read_verdicts: Default::default(),
			dependency_frontier: None,
			wire: lease.wire.clone(),
			node_id: lease.node_id.clone(),
			node: lease.node.clone(),
			node_address: lease.node_address.clone(),
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
			pool: pool.clone(),
			read_run: lease.read_run,
			read_grant: lease.read_grant,
			shared_area: false,
			home_corpus_digest: None,
			environment: lease.environment.clone(),
		})
	}

	pub(crate) async fn resume_inherited(&mut self) -> Result<()> {
		if !self.inherited_lease {
			return Err(Error::Forbidden);
		}
		if !self.tx.is_active() {
			self.identity.check_binding(&self.pool).await?;
			self.tx
				.install(self.pool.begin(&self.identity.tenancy()?).await?);
		}
		Ok(())
	}

	/// Commit the completed authorization checks and release their row locks
	/// while the worker waits on an external provider.
	pub async fn suspend(&mut self) -> Result<()> {
		Authorization::record_many(&mut self.tx, &self.identity.tenant, &self.pending_decisions)
			.await?;
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
			self.tx
				.install(self.pool.begin(&self.identity.tenancy()?).await?);
		}
		self.snapshot = self.identity.lock_with_mode(&mut self.tx, false).await?;
		if let Some(node) = &self.node {
			crate::apps::identity::repositories::nodes::require_live(
				&mut *self.tx,
				&self.identity.tenant,
				node.as_str(),
			)
			.await?;
		}
		self.tx.admissions.clear();
		self.remote_read_cache.clear();
		self.unavailable_peers.clear();
		self.checking_reads.clear();
		self.read_verdicts.clear();
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
		let resource = aidash_application::authorization::workspaces::resource(
			&mut crate::bootstrap::run_visibility_scope(self),
			id,
		)
		.await?;
		let node = self.workspace_node(id).await?;
		self.set_node(node);
		Ok(resource)
	}

	/// Names the Node this authority serves. Approvals proven for another Node
	/// do not carry over.
	pub(crate) fn set_node(&mut self, node: aidash_domain::nodes::NodeName) {
		if self.node.as_ref() != Some(&node) {
			self.approved_catalog.clear();
			self.node_address = Some(format!("{}/{node}", self.node_id));
			self.node = Some(node);
		}
	}

	/// Load an owned Node's policy resource in this authority transaction.
	pub(crate) async fn node_resource(
		&mut self,
		name: &aidash_domain::nodes::NodeName,
	) -> Result<Resource> {
		let node = crate::apps::identity::repositories::nodes::get_live(
			&mut *self.tx,
			&self.identity.tenant,
			name.as_str(),
		)
		.await?;
		Ok(self.resource(
			"node",
			node.name.as_str(),
			json!({"label":node.label,"default":node.default}),
		))
	}

	/// Authorize work on an owned Node using its current policy attributes,
	/// then select its Catalog and Federation identity.
	pub(crate) async fn use_node(&mut self, name: aidash_domain::nodes::NodeName) -> Result<()> {
		let resource = self
			.node_resource(&name)
			.await
			.map_err(|error| match error {
				Error::NotFound(_) => Error::Forbidden,
				other => other,
			})?;
		self.require(
			&resource,
			aidash_domain::identity::tenant_administration::NODE_USE,
		)
		.await?;
		self.set_node(name);
		Ok(())
	}

	/// The Node of a Workspace of this Access's Tenant.
	pub(crate) async fn workspace_node(
		&mut self,
		workspace: Uuid,
	) -> Result<aidash_domain::nodes::NodeName> {
		let name: String = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("node"))
				.from(Alias::new("workspaces"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(workspace)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **self.tx)
		.await?;
		crate::apps::identity::repositories::nodes::require_live(
			&mut *self.tx,
			&self.identity.tenant,
			&name,
		)
		.await?;
		Ok(aidash_domain::nodes::NodeName::parse(&name)?)
	}

	/// The Node whose Catalog Approvals apply: the Workspace's or Run's once an
	/// operation has named it, the Tenant's default Node otherwise.
	pub(crate) async fn catalog_node(&mut self) -> Result<aidash_domain::nodes::NodeName> {
		if let Some(node) = &self.node {
			crate::apps::identity::repositories::nodes::require_live(
				&mut *self.tx,
				&self.identity.tenant,
				node.as_str(),
			)
			.await?;
			return Ok(node.clone());
		}
		let name: String = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("name"))
				.from(Alias::new("nodes"))
				.and_where(
					Expr::col(Alias::new("tenant")).eq(Expr::value(self.identity.tenant.clone())),
				)
				.and_where(Expr::col(Alias::new("is_default")))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **self.tx)
		.await?;
		crate::apps::identity::repositories::nodes::require_live(
			&mut *self.tx,
			&self.identity.tenant,
			&name,
		)
		.await?;
		let node = aidash_domain::nodes::NodeName::parse(&name)?;
		self.set_node(node.clone());
		Ok(node)
	}

	/// The address of the Node whose Catalog Approvals apply.
	pub(crate) async fn catalog_address(&mut self) -> Result<aidash_domain::nodes::NodeAddress> {
		let node = self.catalog_node().await?;
		Ok(aidash_domain::nodes::NodeAddress {
			deployment: aidash_domain::nodes::DeploymentId::parse(&self.node_id)?,
			node,
		})
	}

	pub async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		for subject in &self.subjects {
			super::identity::require_live_subject_on(self.tx.executor(), &self.snapshot, subject)
				.await?;
		}
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
			let mut audit: Box<dyn TransactionExecutor> = self
				.pool
				.begin(&self.identity.tenancy()?)
				.await?
				.into_executor()?;
			AuthorizationDecision::append_many(audit.as_mut(), &self.identity.tenant, records)
				.await?;
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
		if result.is_err() && self.tx.is_active() {
			self.tx.discard_memory_fences();
		}
		let tx = self.tx.take().map(|tx| tx.into_executor()).transpose()?;
		self.complete(tx, result).await
	}

	/// Keep the policy and credential locks while transferring the same physical
	/// transaction to Reinhardt for the protected mutation and its audit.
	pub fn into_native(self) -> Result<NativeAccess> {
		if !self.tx.is_active() {
			return Err(Error::Conflict(
				"authorization transaction is suspended".into(),
			));
		}
		Ok(NativeAccess { access: self })
	}

	async fn complete<T>(
		self,
		tx: Option<Box<dyn TransactionExecutor>>,
		result: Result<T>,
	) -> Result<T> {
		if result.is_ok() {
			let mut tx =
				tx.ok_or_else(|| Error::Conflict("authorization transaction is suspended".into()))?;
			AuthorizationDecision::append_many(
				tx.as_mut(),
				&self.identity.tenant,
				&self.pending_decisions,
			)
			.await?;
			tx.commit().await?;
		} else {
			if let Some(tx) = tx {
				tx.rollback().await?;
			}
			if matches!(result, Err(Error::Forbidden)) {
				// Denials survive rollback with the evaluated policy revision.
				let mut audit: Box<dyn TransactionExecutor> = self
					.pool
					.begin(&self.identity.tenancy()?)
					.await?
					.into_executor()?;
				AuthorizationDecision::append_many(
					audit.as_mut(),
					&self.identity.tenant,
					&self.pending_decisions,
				)
				.await?;
				audit.commit().await?;
			}
		}
		result
	}
}

pub(crate) struct NativeAccess {
	access: Access,
}
// Native ORM readers and memory proofs share the same physical transaction.
// Keeping one owner also lets publication proofs retain the caller's locks.
impl std::ops::Deref for NativeAccess {
	type Target = Access;
	fn deref(&self) -> &Access {
		&self.access
	}
}
impl std::ops::DerefMut for NativeAccess {
	fn deref_mut(&mut self) -> &mut Access {
		&mut self.access
	}
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
		Access::begin(store, identity).await?.into_native()
	}

	pub fn resource(&self, kind: &str, id: impl ToString, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}

	pub async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access.decide(resource, action).await
	}

	pub async fn finish<T>(self, result: Result<T>) -> Result<T> {
		self.access.finish(result).await
	}
	pub(crate) async fn rollback(mut self) -> Result<()> {
		self.access
			.tx
			.take()
			.ok_or_else(|| Error::Conflict("authorization transaction is suspended".into()))?
			.rollback()
			.await
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
		(self.access.tx.as_mut(), &self.access.identity.tenant)
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
	pub(crate) fn remote_transport(
		&self,
	) -> Option<crate::apps::federation::remote::services::wire::Wire> {
		self.access.wire.clone()
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
	/// How a Federation reference names the Node this authority serves: its
	/// address once the Node is known, the Deployment id before.
	pub(crate) fn node_reference(&self) -> &str {
		self.node_address.as_deref().unwrap_or(&self.node_id)
	}

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

#[cfg(test)]
#[path = "access/tests.rs"]
mod tests;
