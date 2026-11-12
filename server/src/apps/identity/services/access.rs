//! A live policy/credential lease used by typed execution boundaries.
use super::{
	Authorization, Snapshot,
	identity::SubjectIdentity,
	policy::{Decision, Effect, Evaluation, PolicyBundle, Resource},
};
use crate::apps::identity::models::{AuthorizationDecision, DecisionOwner, DecisionRow};
use crate::database::native::Pool;
use crate::{Error, Result, store::Store};
use aidash_application::authorization::refusal_note;
use aidash_domain::tool::RefusalCheck;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::{Value, json};
use std::ops::{Deref, DerefMut};
use uuid::Uuid;

#[path = "visibility.rs"]
mod visibility;

pub(super) fn subnode_workspace_write(action: &str) -> bool {
	matches!(
		action,
		"workspace.update"
			| "workspace.share"
			| "task.create"
			| "task.update"
			| "task.claim"
			| "task.delegate"
			| "task.execute"
			| "task.complete"
			| "task.abandon"
			| "artifact.create"
			| "message.create"
			| "message.update"
			| "run.message"
			| "run.control"
			| "run.decline_call"
			| "file.write"
			| "file.delete"
			| "file.restore"
			| "file.receive"
			| "file.transfer"
			| "file.share"
			| "semantic.index.manage"
	)
}

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

/// What makes two audit rows of one Access the same row.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct RecordedKey {
	owner: Option<DecisionOwner>,
	coalesced: bool,
	subject: String,
	action: String,
	kind: String,
	id: String,
	revision: i64,
	allowed: bool,
}

impl RecordedKey {
	fn new(
		owner: Option<DecisionOwner>,
		coalesced: bool,
		input: &Evaluation,
		decision: &Decision,
	) -> Self {
		Self {
			owner,
			coalesced,
			subject: input.subject.clone(),
			action: input.action.clone(),
			kind: input.resource.kind.clone(),
			id: input.resource.id.clone(),
			revision: decision.revision,
			allowed: decision.allowed,
		}
	}
}

/// A repeated read hands over nothing a recorded row does not already say.
fn read_action(action: &str) -> bool {
	action.ends_with(".read") || action == "workspace.events"
}

#[derive(Default)]
pub(crate) struct AccessTransaction {
	transaction: Option<crate::database::native::Transaction>,
	/// Admissions proven on this physical transaction. Their distribution, catalog and
	/// policy locks last exactly as long, so the record ends with the transaction or
	/// with a savepoint rollback that may have released them (its lock epoch).
	admissions: std::collections::BTreeMap<CatalogAdmission, crate::registry::Entry>,
	admissions_epoch: u64,
	// Node deletion takes the policy lock exclusively. A successful check is
	// valid only while this transaction retains its policy lease.
	live_nodes: std::collections::BTreeSet<(String, String)>,
	live_nodes_epoch: u64,
	// One walk reaches the same Workspace once per read. Its owner is held by the
	// share lock when `lock` was asked for, and neither fact changes for a live row.
	workspace_owners: std::collections::BTreeMap<(Uuid, bool), String>,
	workspace_nodes: std::collections::BTreeMap<Uuid, String>,
	workspaces_epoch: u64,
}

impl AccessTransaction {
	fn new(transaction: crate::database::native::Transaction) -> Self {
		Self {
			transaction: Some(transaction),
			admissions: Default::default(),
			admissions_epoch: 0,
			live_nodes: Default::default(),
			live_nodes_epoch: 0,
			workspace_owners: Default::default(),
			workspace_nodes: Default::default(),
			workspaces_epoch: 0,
		}
	}

	pub(crate) fn is_active(&self) -> bool {
		self.transaction.is_some()
	}

	/// The distribution lock this physical transaction holds, read when asked. A
	/// suspended transaction holds none.
	pub(crate) fn distribution_hold(&self) -> crate::database::native::distribution::Hold {
		self.transaction
			.as_ref()
			.map(|transaction| transaction.distribution().clone())
			.unwrap_or_default()
	}

	/// The native transaction, for a trait object that cannot use the wrapper's
	/// `Deref` coercion directly.
	pub(crate) fn native_mut(&mut self) -> &mut crate::database::native::Transaction {
		self
	}

	fn install(&mut self, transaction: crate::database::native::Transaction) {
		debug_assert!(self.transaction.is_none());
		self.admissions.clear();
		self.live_nodes.clear();
		self.forget_workspaces();
		self.transaction = Some(transaction);
	}

	fn forget_workspaces(&mut self) {
		self.workspace_owners.clear();
		self.workspace_nodes.clear();
	}

	/// A savepoint rollback may have released the locks these facts rest on.
	fn current_workspaces(&mut self) {
		let epoch = self
			.transaction
			.as_ref()
			.map_or(0, |transaction| transaction.lock_epoch());
		if epoch != self.workspaces_epoch {
			self.forget_workspaces();
			self.workspaces_epoch = epoch;
		}
	}

	pub(crate) fn workspace_owner(&mut self, id: Uuid, lock: bool) -> Option<String> {
		self.current_workspaces();
		self.workspace_owners.get(&(id, lock)).cloned()
	}

	pub(crate) fn remember_workspace_owner(&mut self, id: Uuid, lock: bool, owner: &str) {
		self.current_workspaces();
		self.workspace_owners.insert((id, lock), owner.to_owned());
	}

	fn take(&mut self) -> Option<crate::database::native::Transaction> {
		self.admissions.clear();
		self.live_nodes.clear();
		self.forget_workspaces();
		self.transaction.take()
	}

	async fn require_live_node(&mut self, tenant: &str, node: &str) -> Result<()> {
		let epoch = self
			.transaction
			.as_ref()
			.ok_or_else(|| Error::Conflict("authorization transaction is suspended".into()))?
			.lock_epoch();
		if epoch != self.live_nodes_epoch {
			self.live_nodes.clear();
			self.live_nodes_epoch = epoch;
		}
		let key = (tenant.to_owned(), node.to_owned());
		if !self.live_nodes.contains(&key) {
			crate::apps::identity::repositories::nodes::require_live(&mut **self, tenant, node)
				.await?;
			self.live_nodes.insert(key);
		}
		Ok(())
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

/// Where an authority's silent-Node and denial counts stood when a call began
/// (see [`Access::call_mark`]).
#[derive(Clone, Copy, Default)]
pub(crate) struct CallMark {
	silent: u64,
	denials: u64,
}

/// What a refusal is read against: everything this authority has seen, or one call's.
#[derive(Clone, Copy)]
enum Scope {
	Authority,
	Call(CallMark),
}

pub(crate) struct Access {
	pub marketplace_audit: Option<serde_json::Value>,
	pub core_gc_complete: bool,
	pub(super) remote_read_cache: std::collections::BTreeMap<
		(Uuid, String),
		aidash_application::ports::federation::registry_reads::RemoteReads,
	>,
	/// Nodes that did not answer a check within this authority (#751). They
	/// are not asked again, and no negative verdict is kept while any is set.
	pub(super) unavailable_peers: std::collections::BTreeSet<String>,
	/// The Node the row being checked gave up on, if any (#770). A list loop
	/// clears it when a row starts and takes it when the row is not visible, so a
	/// refusal is never counted and a silent Node is, once per row.
	pub(crate) row_silence: Option<String>,
	/// Whether this authority serves a list, search or event read, which leaves
	/// a Node alone for a while after it did not answer (#770). A single read, a
	/// Read Recheck and an inference stream always ask.
	pub(crate) lists: bool,
	/// Items a native read loop left out because a Node did not answer, until its
	/// caller takes them (#770). Loops in the application crate count their own.
	pub(crate) unconfirmed: aidash_domain::unconfirmed::Tally,
	/// Whether a `require` of this authority has denied. Such a denial is a
	/// real refusal, so it is never reported as a Node that could not answer.
	pub(super) denied: bool,
	/// How often a check of this authority met a Node that did not answer, and the
	/// Node it met last. One authority serves several calls, so a call tells its own
	/// silence from an earlier call's by comparing these with a [`CallMark`].
	pub(super) silent_events: u64,
	pub(super) silent_last: Option<String>,
	/// How often a `require` of this authority has denied.
	pub(super) denial_events: u64,
	pub(super) checking_reads: aidash_application::authorization::visits::ReadVisits,
	pub(crate) read_verdicts: aidash_application::authorization::visits::ReadVerdicts,
	/// The first read that stopped a walk in this authority boundary.
	pub(crate) read_block:
		Option<aidash_application::authorization::visibility::provenance::ReadBlock>,
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
	/// The Subnode Workspaces a Read Recheck in progress has opened, one entry per
	/// Run it is checking, and what each Run opens.
	pub(crate) recheck_workspaces: Vec<Vec<Uuid>>,
	pub(crate) subnode_workspaces: std::collections::BTreeMap<Uuid, Vec<Uuid>>,
	/// Which Run created the Subnode that owns a Workspace, if any.
	pub(crate) subnode_creators: std::collections::BTreeMap<Uuid, Option<Uuid>>,
	pub tx: AccessTransaction,
	pub identity: SubjectIdentity,
	pub snapshot: Snapshot,
	pub subjects: Vec<String>,
	/// The subject an execution admission appended to the chain, which is the Agent a task is
	/// delegated to. Its refusals are named in the refusal note; the Run's own chain never is.
	pub(crate) delegated_subject: Option<String>,
	pub durable_audit: bool,
	pub audit: bool,
	pub context: Value,
	pub inherited_lease: bool,
	pub approved_catalog: std::collections::BTreeSet<(String, String)>,
	pub cached_runs: std::collections::BTreeMap<(Uuid, Uuid), bool>,
	pub cached_humans: std::collections::BTreeMap<Uuid, bool>,
	pending_decisions: Vec<DecisionRow>,
	/// Rows this Access has queued or written, so a repeat adds none.
	recorded_decisions: std::collections::BTreeSet<RecordedKey>,
	/// The Run a Continuation Check or the loop's Read Recheck is made for, while
	/// it is being made (`rechecking`). Every allow under it is a repeat of the same
	/// question (ADR 0116).
	pub(crate) recheck_owner: Option<DecisionOwner>,
	/// The Run or Remote Grant whose reads are being walked for this reader.
	pub(super) walk_root: Option<DecisionOwner>,
	pub(crate) read_lookups: read_lookups::ReadLookups,
	pub(crate) last_policy_explicit_deny: bool,
	pub(crate) decision_serial: u64,
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
	remote_read_cache: std::collections::BTreeMap<
		(Uuid, String),
		aidash_application::ports::federation::registry_reads::RemoteReads,
	>,
	/// The reader's silent Nodes and its own denial (#751): a mark made under
	/// the publisher's identity must never name a Node to the reader.
	unavailable_peers: std::collections::BTreeSet<String>,
	row_silence: Option<String>,
	denied: bool,
	silent_events: u64,
	silent_last: Option<String>,
	denial_events: u64,
	checking_reads: aidash_application::authorization::visits::ReadVisits,
	read_verdicts: aidash_application::authorization::visits::ReadVerdicts,
	read_block: Option<aidash_application::authorization::visibility::provenance::ReadBlock>,
	dependency_frontier: Option<Vec<super::peer::dependencies::Reference>>,
	recheck_owner: Option<DecisionOwner>,
	walk_root: Option<DecisionOwner>,
	read_lookups: read_lookups::ReadLookups,
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

/// An Access while it makes the Continuation Check or the Read Recheck of one Run
/// (ADR 0116). Every allow made through it repeats the same question. Dropping it
/// ends that, on every path, so what the Access is asked afterwards is recorded as
/// it is today.
pub(crate) struct Rechecking<'a> {
	access: &'a mut Access,
	previous: Option<DecisionOwner>,
}
impl Deref for Rechecking<'_> {
	type Target = Access;
	fn deref(&self) -> &Access {
		self.access
	}
}
impl DerefMut for Rechecking<'_> {
	fn deref_mut(&mut self) -> &mut Access {
		self.access
	}
}
impl Drop for Rechecking<'_> {
	fn drop(&mut self) {
		self.access.recheck_owner = self.previous.take();
	}
}

impl Access {
	/// Check `run` as its Continuation Check or Read Recheck until the guard drops.
	pub(crate) fn rechecking(&mut self, run: Uuid) -> Rechecking<'_> {
		let previous = self.recheck_owner.replace(DecisionOwner::Run(run));
		Rechecking {
			access: self,
			previous,
		}
	}

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
	pub(crate) fn authority_context(&self) -> String {
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
	/// Revalidate saved authority for recovery without auditing or retiring live identities.
	/// The same policy, credential and session locks still delimit the proof.
	pub(crate) async fn begin_proof(store: &Store, identity: &SubjectIdentity) -> Result<Self> {
		identity.check_binding_read_only(&store.pool).await?;
		let mut tx = store.pool.begin(&identity.tenancy()?).await?;
		let snapshot = identity.lock_with_mode(&mut tx, false).await?;
		let mut access = Self::from_transaction(
			tx,
			&store.pool,
			store.deployment.as_str(),
			Some(crate::apps::federation::remote::services::wire::Wire::of_store(store)),
			identity,
			snapshot,
		);
		access.audit = false;
		Ok(access)
	}

	pub async fn begin_exclusive(store: &Store, identity: &SubjectIdentity) -> Result<Self> {
		Self::begin_with_lock(store, identity, true).await
	}

	/// The authority of an Authority Change that needs one of `actions`. A
	/// subject no policy could allow any of them takes the ordinary lock: its
	/// check then refuses it, and the Tenant's work never waits for it (ADR 0121).
	pub async fn begin_change(
		store: &Store,
		identity: &SubjectIdentity,
		actions: &[&str],
	) -> Result<Self> {
		let snapshot = Authorization {
			pool: store.pool.clone(),
		}
		.snapshot(&identity.tenant)
		.await?;
		Self::begin_change_deciding_on(store, identity, actions, &snapshot.bundle).await
	}

	/// [`Self::begin_change`] on the policy `read` before the lock. A policy
	/// committed since can allow one of the actions after all, and a change that
	/// holds the policy only shared would replace it with no flag and no limit,
	/// so that is answered like a stale revision: send the change again.
	async fn begin_change_deciding_on(
		store: &Store,
		identity: &SubjectIdentity,
		actions: &[&str],
		read: &PolicyBundle,
	) -> Result<Self> {
		let may_allow = |bundle: &PolicyBundle| {
			actions
				.iter()
				.any(|action| bundle.may_allow(&identity.subject, action))
		};
		let exclusive = may_allow(read);
		let access = Self::begin_with_lock(store, identity, exclusive).await?;
		if !exclusive && may_allow(&access.snapshot.bundle) {
			return Err(Error::Conflict("authorization revision changed".into()));
		}
		Ok(access)
	}

	async fn begin_with_lock(
		store: &Store,
		identity: &SubjectIdentity,
		exclusive: bool,
	) -> Result<Self> {
		Self::begin_on(
			&store.pool,
			store.deployment.as_str(),
			Some(Self::wire_for(store, exclusive)),
			identity,
			exclusive,
		)
		.await
	}

	/// How an authority that holds `store` reaches other Nodes. One that holds
	/// the policy row exclusively reaches none: the peer's own shared lock on
	/// that row would wait for the lock its caller holds (#751).
	fn wire_for(
		store: &Store,
		exclusive: bool,
	) -> crate::apps::federation::remote::services::wire::Wire {
		use crate::apps::federation::remote::services::wire::Wire;
		if exclusive {
			Wire::without_peers(store)
		} else {
			Wire::of_store(store)
		}
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
			Some(Self::wire_for(store, exclusive)),
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
			row_silence: None,
			lists: false,
			unconfirmed: Default::default(),
			denied: false,
			silent_events: 0,
			silent_last: None,
			denial_events: 0,
			checking_reads: Default::default(),
			read_verdicts: Default::default(),
			read_block: None,
			dependency_frontier: None,
			recheck_workspaces: Default::default(),
			subnode_workspaces: Default::default(),
			subnode_creators: Default::default(),
			wire,
			node_id: node.to_owned(),
			node: None,
			node_address: None,
			tx: AccessTransaction::new(tx),
			identity: identity.clone(),
			snapshot,
			subjects: vec![identity.subject.clone()],
			delegated_subject: None,
			durable_audit: false,
			audit: true,
			context: json!({}),
			inherited_lease: false,
			approved_catalog: Default::default(),
			cached_runs: Default::default(),
			cached_humans: Default::default(),
			pending_decisions: vec![],
			recorded_decisions: Default::default(),
			recheck_owner: None,
			walk_root: None,
			read_lookups: Default::default(),
			last_policy_explicit_deny: false,
			decision_serial: 0,
			pool: pool.clone(),
			read_run: None,
			read_grant: None,
			shared_area: false,
			home_corpus_digest: None,
			environment: json!({"node_id":node,"transport":"api"}),
		}
	}
	pub(crate) async fn flush_audit(&mut self) -> Result<()> {
		let rows = std::mem::take(&mut self.pending_decisions);
		Authorization::record_rows(&mut self.tx, &self.identity.tenant, rows).await?;
		self.written_decisions();
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
			for node in super::identity::subject_nodes(&self.snapshot, subject)? {
				self.tx
					.require_live_node(&self.snapshot.bundle.tenant, node)
					.await?;
			}
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
			unavailable_peers: std::mem::take(&mut self.unavailable_peers),
			row_silence: self.row_silence.take(),
			denied: std::mem::take(&mut self.denied),
			silent_events: std::mem::take(&mut self.silent_events),
			silent_last: self.silent_last.take(),
			denial_events: std::mem::take(&mut self.denial_events),
			checking_reads: std::mem::take(&mut self.checking_reads),
			read_verdicts: std::mem::take(&mut self.read_verdicts),
			read_block: self.read_block.take(),
			dependency_frontier: self.dependency_frontier.take(),
			recheck_owner: self.recheck_owner.take(),
			walk_root: self.walk_root.take(),
			read_lookups: std::mem::take(&mut self.read_lookups),
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
		self.unavailable_peers = old.unavailable_peers;
		self.row_silence = old.row_silence;
		self.denied = old.denied;
		self.silent_events = old.silent_events;
		self.silent_last = old.silent_last;
		self.denial_events = old.denial_events;
		self.checking_reads = old.checking_reads;
		self.read_verdicts = old.read_verdicts;
		self.read_block = old.read_block;
		self.dependency_frontier = old.dependency_frontier;
		self.recheck_owner = old.recheck_owner;
		self.walk_root = old.walk_root;
		self.read_lookups = old.read_lookups;
	}

	// The caller retains the outer Access until this mutation commits. Reusing
	// its locks avoids queuing a second shared lock behind a waiting revoker.
	pub async fn under_lease(lease: &Self) -> Result<Self> {
		Self::under_lease_on(lease, &lease.pool).await
	}
	pub(crate) async fn under_lease_on(lease: &Self, pool: &Pool) -> Result<Self> {
		let mut tx = pool.begin(&lease.identity.tenancy()?).await?;
		// The lease retains its distribution lock until this authority is done with it.
		tx.inherit_distribution(lease.tx.distribution_hold());
		Ok(Self {
			marketplace_audit: None,
			core_gc_complete: false,
			remote_read_cache: Default::default(),
			unavailable_peers: Default::default(),
			row_silence: None,
			lists: false,
			unconfirmed: Default::default(),
			denied: false,
			silent_events: 0,
			silent_last: None,
			denial_events: 0,
			checking_reads: Default::default(),
			read_verdicts: Default::default(),
			read_block: None,
			dependency_frontier: None,
			recheck_workspaces: Default::default(),
			subnode_workspaces: Default::default(),
			subnode_creators: Default::default(),
			wire: lease.wire.clone(),
			node_id: lease.node_id.clone(),
			node: lease.node.clone(),
			node_address: lease.node_address.clone(),
			tx: AccessTransaction::new(tx),
			identity: lease.identity.clone(),
			snapshot: lease.snapshot.clone(),
			subjects: lease.subjects.clone(),
			delegated_subject: None,
			durable_audit: false,
			audit: true,
			context: lease.context.clone(),
			inherited_lease: true,
			approved_catalog: lease.approved_catalog.clone(),
			cached_runs: Default::default(),
			cached_humans: Default::default(),
			pending_decisions: vec![],
			recorded_decisions: Default::default(),
			recheck_owner: None,
			walk_root: None,
			read_lookups: Default::default(),
			last_policy_explicit_deny: false,
			decision_serial: 0,
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
		let rows = std::mem::take(&mut self.pending_decisions);
		Authorization::record_rows(&mut self.tx, &self.identity.tenant, rows).await?;
		let previous = self
			.tx
			.take()
			.ok_or_else(|| Error::Conflict("authorization transaction is suspended".into()))?;
		previous.commit().await?;
		self.written_decisions();
		Ok(())
	}

	/// Start a fresh execution authorization boundary after an external wait.
	pub async fn refresh_execution(&mut self, run_id: Uuid) -> Result<()> {
		if self.durable_audit {
			self.commit_durable(vec![]).await?;
		}
		if !self.tx.is_active() {
			self.tx
				.install(self.pool.begin(&self.identity.tenancy()?).await?);
		}
		self.snapshot = self.identity.lock_with_mode(&mut self.tx, false).await?;
		self.tx.live_nodes.clear();
		if let Some(node) = &self.node {
			self.tx
				.require_live_node(&self.identity.tenant, node.as_str())
				.await?;
		}
		let policy = self.pool.dashboard_policy();
		crate::apps::identity::models::DashboardExecutionOrigin::require_current(
			&mut **self.tx,
			run_id,
			policy.as_ref(),
		)
		.await?;
		self.tx.admissions.clear();
		self.remote_read_cache.clear();
		self.unavailable_peers.clear();
		self.row_silence = None;
		self.denied = false;
		self.checking_reads.clear();
		self.read_verdicts.clear();
		self.read_block = None;
		self.read_lookups = Default::default();
		self.walk_root = None;
		self.recheck_owner = None;
		self.dependency_frontier = None;
		self.subjects = vec![self.identity.subject.clone()];
		self.delegated_subject = None;
		self.durable_audit = true;
		self.audit = true;
		self.context = json!({});
		self.inherited_lease = false;
		self.approved_catalog.clear();
		self.cached_runs.clear();
		self.cached_humans.clear();
		self.pending_decisions.clear();
		self.written_decisions();
		self.last_policy_explicit_deny = false;
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
		self.recorded_decisions.clear();
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
		self.tx.current_workspaces();
		let name: String = match self.tx.workspace_nodes.get(&workspace) {
			Some(name) => name.clone(),
			None => {
				let name: String = crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("node"))
						.from(Alias::new("workspaces"))
						.and_where(Expr::col(Alias::new("id")).eq(Expr::value(workspace)))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_one(&mut **self.tx)
				.await?;
				self.tx.workspace_nodes.insert(workspace, name.clone());
				name
			}
		};
		self.tx
			.require_live_node(&self.identity.tenant, &name)
			.await?;
		Ok(aidash_domain::nodes::NodeName::parse(&name)?)
	}

	/// The Node whose Catalog Approvals apply: the Workspace's or Run's once an
	/// operation has named it, the Tenant's default Node otherwise.
	pub(crate) async fn catalog_node(&mut self) -> Result<aidash_domain::nodes::NodeName> {
		if let Some(node) = &self.node {
			self.tx
				.require_live_node(&self.identity.tenant, node.as_str())
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
		self.tx
			.require_live_node(&self.identity.tenant, &name)
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

	/// A dissolved Subnode Workspace is retained for reading and citations.
	/// User mutations hold a shared Subnode lock until their transaction commits;
	/// dissolution takes the exclusive lock. A Run's writes are not checked here
	/// (see `decide`): each path that writes for a Run must call
	/// `subnodes::require_writable` first in its own transaction.
	pub(crate) async fn ensure_subnode_writable(&mut self, workspace: Uuid) -> Result<()> {
		use reinhardt::db::orm::{QueryRow, execution::convert_values};
		use reinhardt::query::LockType;
		let (sql, values) = Query::select()
			.column(Alias::new("state"))
			.from(Alias::new("subnodes"))
			.and_where(Expr::col("tenant").eq(self.identity.tenant.clone()))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.lock(LockType::Share)
			.build(PostgresQueryBuilder);
		if let Some(row) = self
			.tx
			.native_mut()
			.fetch_optional(&sql, convert_values(values))
			.await?
		{
			let row = QueryRow::from_backend_row(row);
			let state: String = row.get("state").ok_or(crate::Error::Forbidden)?;
			if state != "active" {
				return Err(Error::Forbidden);
			}
		}
		Ok(())
	}

	pub async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.decide_with_cause(resource, action)
			.await
			.map(|verdict| verdict == Verdict::Allowed)
	}
	/// [`Self::decide`], and whether a refusal comes from the Subnode read-only rule rather than
	/// from the Tenant's policy. The rule is structural: no change of policy lifts it.
	async fn decide_with_cause(&mut self, resource: &Resource, action: &str) -> Result<Verdict> {
		// Within one authority, a repeated allow needs no second evaluation. A
		// denial is evaluated and recorded every time. So is a write a Subnode can
		// fence: the fence closes between two identical checks.
		let repeat = (self.audit_class(action).1 && !subnode_workspace_write(action))
			.then(|| self.repeat_key(resource, action));
		if let Some(key) = &repeat
			&& self.read_lookups.allowed.contains(key)
		{
			self.decision_serial = self
				.decision_serial
				.saturating_add(self.subjects.len() as u64);
			self.last_policy_explicit_deny = false;
			return Ok(Verdict::Allowed);
		}
		for subject in &self.subjects {
			// Unknown subjects reach evaluation so their denial is audited.
			if self.snapshot.bundle.subjects.contains_key(subject) {
				for node in super::identity::subject_nodes(&self.snapshot, subject)? {
					self.tx
						.require_live_node(&self.snapshot.bundle.tenant, node)
						.await?;
				}
			}
		}
		// A Subnode creator reads, but never writes, the Subnode's Workspace.
		if self.inherited_lease && subnode_workspace_write(action) {
			let workspace = resource
				.attributes
				.get("workspace_id")
				.and_then(Value::as_str)
				.or_else(|| (resource.kind == "workspace").then_some(resource.id.as_str()))
				.and_then(|id| Uuid::parse_str(id).ok());
			if workspace.is_some_and(|workspace| {
				aidash_application::authorization::workspaces::subnode_read_workspace(
					&self.context,
					workspace,
				)
			}) {
				return Ok(Verdict::SubnodeReadOnly);
			}
		}
		if self.read_run.is_none() && subnode_workspace_write(action) {
			let workspace = resource
				.attributes
				.get("workspace_id")
				.and_then(Value::as_str)
				.or_else(|| (resource.kind == "workspace").then_some(resource.id.as_str()))
				.and_then(|id| Uuid::parse_str(id).ok());
			if let Some(workspace) = workspace {
				self.ensure_subnode_writable(workspace).await?;
			}
		}
		let allowed = aidash_application::authorization::lease::decide(
			&mut crate::bootstrap::run_visibility_scope(self),
			resource,
			action,
		)
		.await?;
		if allowed && let Some(key) = repeat {
			self.read_lookups.allowed.insert(key);
		}
		Ok(if allowed {
			Verdict::Allowed
		} else {
			Verdict::Denied
		})
	}

	/// Everything an evaluation depends on besides the policy revision's bundle.
	fn repeat_key(&self, resource: &Resource, action: &str) -> String {
		json!({
			"revision": self.snapshot.revision,
			"subjects": self.subjects,
			"environment": self.environment,
			"action": action,
			"resource": resource,
		})
		.to_string()
	}

	/// Who a check is made for and whether a repeated allow adds no row. Checks
	/// that start something stay on the record every time (ADR 0116).
	fn audit_class(&self, action: &str) -> (Option<DecisionOwner>, bool) {
		if let Some(owner) = self.recheck_owner {
			return (Some(owner), true);
		}
		let walking = !self.checking_reads.is_idle();
		let worker = match (self.read_run, self.read_grant) {
			(Some(run), _) => Some(DecisionOwner::Run(run)),
			(None, Some(grant)) => Some(DecisionOwner::RemoteGrant(grant)),
			(None, None) => None,
		};
		if let Some(owner) = worker {
			return (Some(owner), walking || read_action(action));
		}
		match self.walk_root {
			Some(root) if walking => (Some(root), true),
			_ => (None, false),
		}
	}

	/// Non-coalesced rows may be written again after a write, as an SSE frame does.
	fn written_decisions(&mut self) {
		self.recorded_decisions.retain(|key| key.coalesced);
	}

	/// Mark the start of a walk over a reader's dependencies. Returns whether
	/// this call owns the walk and so must end it.
	pub(crate) fn begin_read_walk(&mut self, owner: DecisionOwner, outermost: bool) -> bool {
		if !outermost && self.walk_root.is_some() {
			return false;
		}
		self.walk_root = Some(owner);
		let authority = self.authority_context();
		self.read_lookups.start_walk(authority);
		true
	}

	/// End a walk. Rows it loaded must not outlive it, because this authority may
	/// go on to write. A worker's new rows commit here, once, before any effect.
	pub(crate) async fn end_read_walk(
		&mut self,
		started: bool,
		result: Result<bool>,
	) -> Result<bool> {
		if started {
			self.walk_root = None;
			self.read_lookups.end_walk(matches!(result, Ok(true)));
			if self.durable_audit && result.is_ok() {
				self.commit_durable(vec![]).await?;
			}
		}
		result
	}

	/// Commit queued rows and `immediate` on the audit's own connection. Rows
	/// already recorded are found in the transaction this authority already
	/// holds, so a repeat opens no connection and takes no audit lock.
	async fn commit_durable(&mut self, immediate: Vec<DecisionRow>) -> Result<()> {
		let mut rows = std::mem::take(&mut self.pending_decisions);
		rows.extend(immediate);
		if rows.is_empty() {
			return Ok(());
		}
		let rows = if self.tx.is_active() {
			AuthorizationDecision::unrecorded(&mut **self.tx, &self.identity.tenant, rows).await?
		} else {
			rows
		};
		if rows.is_empty() {
			return Ok(());
		}
		let mut audit: Box<dyn TransactionExecutor> = self
			.pool
			.begin(&self.identity.tenancy()?)
			.await?
			.into_executor()
			.await?;
		AuthorizationDecision::append_rows(audit.as_mut(), &self.identity.tenant, &rows).await?;
		audit.commit().await?;
		Ok(())
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

	pub(crate) fn policy_explicit_deny(bundle: &PolicyBundle, decision: &Decision) -> bool {
		// Delegation failures retain the ancestor's matched policy IDs, but replace
		// its reason. A missing ancestor allow must remain an approval request.
		!decision.allowed
			&& (decision.reason == "explicit_deny"
				|| (decision.reason == "delegation_denied"
					&& bundle.policies.iter().any(|policy| {
						policy.effect == Effect::Deny
							&& decision.matched_policies.contains(&policy.id)
					})))
	}

	pub(crate) async fn record(&mut self, records: &[(Evaluation, Decision)]) -> Result<()> {
		self.decision_serial = self.decision_serial.saturating_add(records.len() as u64);
		self.last_policy_explicit_deny = records
			.iter()
			.any(|(_, decision)| Self::policy_explicit_deny(&self.snapshot.bundle, decision));
		if !self.audit {
			return Ok(());
		}
		let mut immediate = vec![];
		for (input, decision) in records {
			let (owner, may_coalesce) = self.audit_class(&input.action);
			let coalesced = may_coalesce && decision.allowed;
			// A durable worker check stands on its own. Otherwise one request
			// writes one row per subject, action, resource and outcome.
			if (coalesced || !self.durable_audit)
				&& !self
					.recorded_decisions
					.insert(RecordedKey::new(owner, coalesced, input, decision))
			{
				continue;
			}
			let row = DecisionRow {
				input: input.clone(),
				decision: decision.clone(),
				owner,
				coalesced,
			};
			if self.durable_audit && !coalesced {
				immediate.push(row);
			} else {
				self.pending_decisions.push(row);
			}
		}
		// A worker step spans the existing invocation-start/effect/result
		// commits. Keep authority locked for that entire boundary, and commit
		// its decision audit before effects so a killed worker cannot lose it.
		if !immediate.is_empty() {
			self.commit_durable(immediate).await?;
		}
		Ok(())
	}

	pub async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		match self.decide_with_cause(resource, action).await? {
			Verdict::Allowed => {
				// The last check wins: an allow supersedes an earlier policy refusal.
				refusal_note::clear_policy();
				Ok(())
			}
			// A structural refusal is no policy denial: it reaches the model as a Rejected Call.
			Verdict::SubnodeReadOnly => {
				self.denied = true;
				self.denial_events += 1;
				Err(Error::Forbidden)
			}
			Verdict::Denied => {
				self.denied = true;
				self.denial_events += 1;
				self.note_denial(resource, action);
				Err(Error::Forbidden)
			}
		}
	}

	/// [`Self::finish`] for a yes/no answer to one person's request. A `false`
	/// reached while a Node did not answer is that Node's silence, so it is
	/// `NodeUnreachable` and not a refusal (#751).
	pub(crate) async fn finish_deciding(self, result: Result<bool>) -> Result<bool> {
		let silent = match (&result, self.named(Error::Forbidden)) {
			(Ok(false), Error::NodeUnreachable { node, .. }) => Some(node),
			_ => None,
		};
		let decided = self.finish(result).await?;
		match silent {
			Some(node) => Err(Error::NodeUnreachable {
				node,
				after_effect: false,
			}),
			None => Ok(decided),
		}
	}

	/// The Nodes this authority found silent, taken so that a publisher view
	/// can hand them back to the reader it ran for (#751).
	pub(crate) fn take_peer_marks(&mut self) -> std::collections::BTreeSet<String> {
		std::mem::take(&mut self.unavailable_peers)
	}

	/// Mark Nodes that another identity of this authority found silent, but
	/// only those this reader may discover: to any other reader a silent Node
	/// is the plain refusal it always was. With discover decided, the row being
	/// checked is unconfirmed under the adopted Node, so a list counts it (#770);
	/// the publisher's check already wrote the process's memory of it.
	pub(crate) async fn adopt_silent_peers(
		&mut self,
		nodes: impl IntoIterator<Item = String>,
	) -> Result<()> {
		for node in nodes {
			let resource = self.resource("node", &node, json!({"remote_node":node}));
			if self.decide(&resource, "federation.discover").await? {
				self.skip_peer(&node);
			}
		}
		Ok(())
	}

	/// The error for a Run whose graph a memory step found invalid, for the call
	/// that began at `mark`. `Authority` and `Invalidated` both come from a read
	/// that was not confirmed, so when this call's own checks met a Node that did
	/// not answer they name that Node (#751). Otherwise the mapping is the old
	/// one. Limit: after such a meeting, an `Authority` that is a real refusal
	/// reads as silence; the Run still stops and Resume finds out.
	pub(crate) fn memory_failure_since(
		&self,
		mark: CallMark,
		reason: aidash_domain::semantic::Failure,
	) -> Error {
		use aidash_domain::semantic::Failure;
		match reason {
			Failure::Authority => self.named_since(mark, Error::Forbidden),
			Failure::Invalidated => match self.named_since(mark, Error::Forbidden) {
				silent @ Error::NodeUnreachable { .. } => silent,
				_ => Error::RemoteSemantic(Failure::Invalidated),
			},
			reason => Error::RemoteSemantic(reason),
		}
	}

	/// What an error of this authority says to its caller. A `Forbidden` that
	/// follows a Node that did not answer, with no `require` of this authority
	/// having denied, is that Node's silence and not a refusal (#751). For an
	/// authority that serves one call, such as the one behind a single request.
	pub(crate) fn named(&self, error: Error) -> Error {
		self.named_in(Scope::Authority, error)
	}

	/// Where this authority's counts stand as a call begins.
	pub(crate) fn call_mark(&self) -> CallMark {
		CallMark {
			silent: self.silent_events,
			denials: self.denial_events,
		}
	}

	/// [`Self::named`] for a call that shares this authority with others, such as
	/// one Tool Batch of a worker. A refusal of this call is that Node's silence
	/// only if this call's own checks met the Node, and no `require` of this call
	/// denied. A Node an earlier call found silent, or a denial it made, says
	/// nothing about this call's refusal: it would be retried as undelivered
	/// instead of reaching the model as the refusal it is (#751).
	pub(crate) fn named_since(&self, mark: CallMark, error: Error) -> Error {
		self.named_in(Scope::Call(mark), error)
	}

	fn named_in(&self, scope: Scope, error: Error) -> Error {
		let silent = match scope {
			Scope::Authority => (!self.denied)
				.then(|| self.unavailable_peers.first())
				.flatten(),
			Scope::Call(mark) => (self.silent_events > mark.silent
				&& self.denial_events == mark.denials)
				.then_some(self.silent_last.as_ref())
				.flatten(),
		};
		match (&error, silent) {
			(Error::Forbidden, Some(node)) => Error::NodeUnreachable {
				node: node.clone(),
				after_effect: false,
			},
			_ => error,
		}
	}

	/// Leave the call's refusal note for a denial. `decide` does not, because a task that is
	/// not visible reads as a Rejected Call, not an Authority Pause.
	fn note_denial(&self, resource: &Resource, action: &str) {
		let subject = self.delegated_subject.as_ref().filter(|delegated| {
			let input = self.evaluation(delegated, resource, action);
			!self.snapshot.bundle.evaluate(&input).allowed
		});
		refusal_note::set(
			refusal_note::Note::new(RefusalCheck::Policy, Some(action)).subject(subject.cloned()),
		);
	}

	pub async fn finish<T>(mut self, result: Result<T>) -> Result<T> {
		if result.is_err() && self.tx.is_active() {
			self.tx.discard_memory_fences();
		}
		let tx = match self.tx.take() {
			Some(tx) => Some(tx.into_executor().await?),
			None => None,
		};
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
		mut self,
		tx: Option<Box<dyn TransactionExecutor>>,
		result: Result<T>,
	) -> Result<T> {
		if result.is_ok() {
			let mut tx =
				tx.ok_or_else(|| Error::Conflict("authorization transaction is suspended".into()))?;
			let rows = std::mem::take(&mut self.pending_decisions);
			AuthorizationDecision::append_unrecorded(tx.as_mut(), &self.identity.tenant, rows)
				.await?;
			tx.commit().await?;
		} else {
			if let Some(tx) = tx {
				tx.rollback().await?;
			}
			// A denied decision survives whatever error the caller ends with, so a refusal that
			// reached the caller as another error can still be explained afterwards.
			let denied = self
				.pending_decisions
				.iter()
				.any(|row| !row.decision.allowed);
			if denied
				|| matches!(
					result,
					Err(Error::Forbidden
						| Error::AuthorityRefused(_)
						| Error::RemoteExecutionRefused(_)
						| Error::ResumeBlocked(_)
						| Error::RunHidden(_))
				) {
				// Denials survive rollback with the evaluated policy revision.
				let mut audit: Box<dyn TransactionExecutor> = self
					.pool
					.begin(&self.identity.tenancy()?)
					.await?
					.into_executor()
					.await?;
				let rows = std::mem::take(&mut self.pending_decisions);
				AuthorizationDecision::append_unrecorded(
					audit.as_mut(),
					&self.identity.tenant,
					rows,
				)
				.await?;
				audit.commit().await?;
			}
		}
		// The audit rule above looked at the original error; the caller sees
		// which Node could not answer.
		result.map_err(|error| self.named(error))
	}
}

/// The outcome of one authorization check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
	Allowed,
	/// The Tenant's policy denied it.
	Denied,
	/// The Subnode read-only rule refused it, which no policy lifts.
	SubnodeReadOnly,
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
		self.access.require(resource, action).await
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
	/// No check of any kind is in progress.
	pub(crate) fn reads_idle(&self) -> bool {
		self.checking_reads.is_idle()
	}

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
}

impl NativeAccess {
	pub(crate) fn remote_identity(&self) -> (&str, &str) {
		(&self.access.identity.tenant, &self.access.identity.subject)
	}
	pub(crate) fn reach(&self) -> crate::apps::federation::remote::services::wire::Reach {
		self.access.reach()
	}
	pub(crate) fn cached_remote_read(
		&self,
		run: Uuid,
	) -> Option<aidash_application::ports::federation::registry_reads::RemoteReads> {
		self.access.cached_remote_read(run)
	}
	pub(crate) fn remember_remote_read(
		&mut self,
		run: Uuid,
		reads: aidash_application::ports::federation::registry_reads::RemoteReads,
	) {
		self.access.remember_remote_read(run, reads);
	}
	pub(crate) fn peer_unavailable(&self, node: &str) -> bool {
		self.access.peer_unavailable(node)
	}
	pub(crate) fn mark_peer_unavailable(&mut self, node: &str) {
		self.access.mark_peer_unavailable(node);
	}
	pub(crate) fn skip_peer(&mut self, node: &str) {
		self.access.skip_peer(node);
	}
	pub(crate) fn peer_answered(&mut self, node: &str) {
		self.access.peer_answered(node);
	}
}

impl Access {
	/// How this authority asks other Nodes: through its transport, inside the
	/// distribution lock its transaction holds, so that a Node of this Deployment
	/// reached in process shares that lock and does not queue for it again (#751).
	pub(crate) fn reach(&self) -> crate::apps::federation::remote::services::wire::Reach {
		crate::apps::federation::remote::services::wire::Reach(
			self.wire.clone(),
			self.tx.distribution_hold(),
		)
	}
	pub(crate) fn cached_remote_read(
		&self,
		run: Uuid,
	) -> Option<aidash_application::ports::federation::registry_reads::RemoteReads> {
		self.remote_read_cache
			.get(&(run, self.authority_context()))
			.cloned()
	}
	pub(crate) fn remember_remote_read(
		&mut self,
		run: Uuid,
		reads: aidash_application::ports::federation::registry_reads::RemoteReads,
	) {
		// A refusal reached while a Node did not answer may be that Node's
		// silence, so it is asked again and not remembered (#751).
		if reads != aidash_application::ports::federation::registry_reads::RemoteReads::Visible
			&& !self.unavailable_peers.is_empty()
		{
			return;
		}
		self.remote_read_cache
			.insert((run, self.authority_context()), reads);
	}
	pub(crate) fn peer_unavailable(&self, node: &str) -> bool {
		self.unavailable_peers.contains(node) || self.recently_silent(node)
	}
	/// A check met a Node that did not answer: now, or earlier in this authority,
	/// when it was not asked again. Each meeting counts for the call that made it.
	pub(crate) fn mark_peer_unavailable(&mut self, node: &str) {
		self.skip_peer(node);
		if let Some(silent) = self.silence_memory() {
			silent.note(&self.identity.tenant, node);
		}
	}
	/// The row being checked stops at a Node known to be silent, without asking
	/// it again. Unlike a mark, this does not extend the process's memory.
	pub(crate) fn skip_peer(&mut self, node: &str) {
		self.unavailable_peers.insert(node.into());
		self.silent_events += 1;
		self.silent_last = Some(node.into());
		self.row_silence.get_or_insert_with(|| node.into());
	}
	/// A Node answered, with a verdict or a refusal.
	pub(crate) fn peer_answered(&mut self, node: &str) {
		if let Some(silent) = self.wire.as_ref().map(|wire| &wire.silent) {
			silent.clear(&self.identity.tenant, node);
		}
	}
	/// The process's memory of silent Nodes, only for an authority that can ask
	/// another Node at all: one built without peers asks nobody, so it neither
	/// reads the memory nor blames a Node nobody asked (#751).
	fn silence_memory(
		&self,
	) -> Option<&crate::apps::federation::remote::services::wire::SilentNodes> {
		self.wire
			.as_ref()
			.filter(|wire| wire.runtime.is_some())
			.map(|wire| wire.silent.as_ref())
	}
	fn recently_silent(&self, node: &str) -> bool {
		self.lists
			&& self
				.silence_memory()
				.is_some_and(|silent| silent.recent(&self.identity.tenant, node))
	}
	/// A native read loop leaves one item out because `node` did not answer.
	pub(crate) fn leave_out(
		&mut self,
		node: &str,
		kind: aidash_domain::unconfirmed::Kind,
		workspace: Uuid,
	) {
		self.unconfirmed.record(node, kind, workspace);
	}
	/// What native read loops have left out since the last call.
	pub(crate) fn take_unconfirmed(&mut self) -> aidash_domain::unconfirmed::Tally {
		std::mem::take(&mut self.unconfirmed)
	}
	/// A list loop's row begins.
	pub(crate) fn begin_row(&mut self) {
		self.row_silence = None;
	}
	/// The Node the row being checked gave up on, if any.
	pub(crate) fn row_silence(&self) -> Option<String> {
		self.row_silence.clone()
	}
	/// The Node the row gave up on, after its check.
	pub(crate) fn take_row_silence(&mut self) -> Option<String> {
		self.row_silence.take()
	}
	/// Put back, or clear, the Node the row gave up on.
	pub(crate) fn set_row_silence(&mut self, node: Option<String>) {
		self.row_silence = node;
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

#[path = "access/read_lookups.rs"]
pub(crate) mod read_lookups;

#[cfg(test)]
#[path = "access/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "access/read_recheck_tests.rs"]
mod read_recheck_tests;
