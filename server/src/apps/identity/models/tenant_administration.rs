//! Dashboard administration actors. A Tenant Administrator's authority is its
//! Tenant's policy, checked on the same transaction that applies the effect.
use super::{
	AuthorizationDecision, DashboardAdministrationHistory, DashboardIdentity, DashboardMapping,
	DashboardRegistrationRequest,
};
use crate::apps::identity::serializers::tenant_administration::{
	Membership, MembershipUpdate, TenantAdministration, TenantIdentity, TenantMapping,
	TenantRegistration,
};
use crate::authorization::{Snapshot, identity::SubjectIdentity};
use crate::database::native::Pool;
use crate::{Error, Result};
use aidash_application::ports::authorization::dashboard::AccountPolicy;
use aidash_application::tenancy::TenancyScope;
use aidash_domain::identity::tenant_administration as rules;
use aidash_domain::policy::{Decision, Evaluation, Resource};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{AtomicTransaction, Model, QuerySet};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoValue, JoinType, PostgresQueryBuilder, Query,
	QueryStatementBuilder, TableRef,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Who decided a Registration Request or disabled a Mapping.
#[derive(Clone)]
pub(crate) enum AdministrationActor {
	/// The deployment's Deployment Administrator bearer token.
	DeploymentAdministratorBearer,
	/// A browser session holding an enabled Deployment Administrator grant.
	DeploymentAdministrator {
		identity: Uuid,
	},
	TenantAdministrator(Arc<TenantAdministrator>),
	/// An External Identity acting for itself, such as creating a Tenant,
	/// redeeming an Invitation or relabeling its Tenant through a Mapping.
	Identity {
		identity: Uuid,
		mapping: Option<Uuid>,
	},
	/// A Demo Reservation, spent by the sign-in or the load that maps its
	/// External Identity (ADR 0079). Nobody decides it.
	DemoData,
}

/// The fields of one administration history row other than its actor.
pub(crate) struct AdministrationEvent<'a> {
	pub action: &'a str,
	pub identity: Option<Uuid>,
	pub registration: Option<Uuid>,
	pub mapping: Option<Uuid>,
	pub tenant: Option<&'a str>,
	pub subject: Option<&'a str>,
	pub invitation: Option<Uuid>,
}

impl AdministrationActor {
	/// Retains the existing `decision_actor` format of Registration Requests.
	pub(crate) fn decision_actor(&self) -> String {
		match self {
			Self::DeploymentAdministratorBearer => "deployment-administrator-bearer".into(),
			Self::DeploymentAdministrator { identity } => format!("oidc:{identity}"),
			Self::TenantAdministrator(administrator) => {
				format!("oidc:{}", administrator.identity_id)
			}
			Self::Identity { identity, .. } => format!("oidc:{identity}"),
			Self::DemoData => "demo-data".into(),
		}
	}

	pub(crate) fn tenant_administrator(&self) -> Option<&TenantAdministrator> {
		match self {
			Self::TenantAdministrator(administrator) => Some(administrator),
			_ => None,
		}
	}

	fn parts(&self) -> (&'static str, Option<Uuid>, Option<Uuid>) {
		match self {
			Self::DeploymentAdministratorBearer => ("deployment_administrator", None, None),
			Self::DeploymentAdministrator { identity } => {
				("deployment_administrator", Some(*identity), None)
			}
			Self::TenantAdministrator(administrator) => (
				"tenant_administrator",
				Some(administrator.identity_id),
				Some(administrator.mapping_id),
			),
			Self::Identity { identity, mapping } => ("identity", Some(*identity), *mapping),
			Self::DemoData => ("demo_data", None, None),
		}
	}

	/// The actor kind and acting External Identity, as Invitations record them.
	pub(crate) fn kind_and_identity(&self) -> (&'static str, Option<Uuid>) {
		let (kind, identity, _) = self.parts();
		(kind, identity)
	}

	/// Appends one row to the administration history in the caller's transaction.
	pub(crate) async fn record(
		&self,
		tx: &mut dyn TransactionExecutor,
		event: AdministrationEvent<'_>,
	) -> Result<()> {
		self.record_target(tx, event, None).await
	}

	/// Node audit keeps the permanent name claim after the Node is purged.
	pub(crate) async fn record_node(
		&self,
		tx: &mut dyn TransactionExecutor,
		event: AdministrationEvent<'_>,
		node: &str,
	) -> Result<()> {
		self.record_target(tx, event, Some(node)).await
	}

	async fn record_target(
		&self,
		tx: &mut dyn TransactionExecutor,
		event: AdministrationEvent<'_>,
		node: Option<&str>,
	) -> Result<()> {
		let (kind, identity, mapping) = self.parts();
		let occurred_at = super::dashboard_administration::database_time(tx).await?;
		let (sql, values) = Query::insert()
			.into_table(Alias::new(DashboardAdministrationHistory::table_name()))
			.columns(
				[
					"id",
					"occurred_at",
					"action",
					"identity_id",
					"registration_id",
					"mapping_id",
					"tenant",
					"subject",
					"actor_kind",
					"actor_identity_id",
					"actor_mapping_id",
					"invitation_id",
					"node",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value(Uuid::new_v4()),
				IntoValue::into_value(occurred_at),
				IntoValue::into_value(event.action.to_owned()),
				IntoValue::into_value(event.identity),
				IntoValue::into_value(event.registration),
				IntoValue::into_value(event.mapping),
				IntoValue::into_value(event.tenant.map(str::to_owned)),
				IntoValue::into_value(event.subject.map(str::to_owned)),
				IntoValue::into_value(kind),
				IntoValue::into_value(identity),
				IntoValue::into_value(mapping),
				IntoValue::into_value(event.invitation),
				IntoValue::into_value(node.map(str::to_owned)),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}
}

/// An External Identity acting through one of its Mappings. It needs no Tenant
/// Binding (ADR 0024); a Binding only addresses Registration Requests to its
/// Tenant and discloses their Display Attributes.
pub(crate) struct TenantAdministrator {
	pub identity: SubjectIdentity,
	pub identity_id: Uuid,
	pub mapping_id: Uuid,
	node_id: String,
	/// The issuer whose External Identities are restricted to this Tenant.
	restricted: Option<String>,
	policy: AccountPolicy,
	denial: Mutex<Option<(Evaluation, Decision)>>,
}

impl TenantAdministrator {
	pub(crate) fn new(
		identity: SubjectIdentity,
		identity_id: Uuid,
		mapping_id: Uuid,
		node_id: String,
		policy: Option<AccountPolicy>,
	) -> Result<Self> {
		let policy = policy.ok_or(Error::Forbidden)?;
		let restricted = policy.issuers.restricted_issuer(&identity.tenant);
		Ok(Self {
			identity,
			identity_id,
			mapping_id,
			node_id,
			restricted,
			policy,
			denial: Mutex::new(None),
		})
	}

	/// Every effect of a Tenant Administrator happens in its Tenant's scope.
	pub(crate) fn scope(&self) -> Result<TenancyScope> {
		Ok(TenancyScope::tenant(self.tenant())?)
	}

	pub(crate) fn tenant(&self) -> &str {
		&self.identity.tenant
	}

	/// Whether this administrator may decide `registration` from `identity`: it
	/// is addressed to this Tenant, through a Tenant Binding or an Invitation,
	/// and comes from an admitted issuer whose identities may be mapped here,
	/// so an unrestricted identity's Invitation into a bound Tenant counts.
	/// Requests addressed to no Tenant are decided only by Deployment
	/// Administrators.
	pub(crate) fn sees(
		&self,
		registration: &DashboardRegistrationRequest,
		identity: &DashboardIdentity,
	) -> bool {
		registration.tenant.as_deref() == Some(self.tenant())
			&& self
				.policy
				.require_mapping(&identity.issuer, self.tenant())
				.is_ok()
	}

	/// Narrows `identities` in SQL to the admitted issuers whose identities may
	/// be mapped into this Tenant. Callers pass only identities already mapped
	/// into this Tenant or addressed to it.
	fn visible(&self, identities: QuerySet<DashboardIdentity>) -> QuerySet<DashboardIdentity> {
		let issuers: Vec<String> = self
			.policy
			.issuers
			.admitted()
			.into_iter()
			.filter(|issuer| self.policy.require_mapping(issuer, self.tenant()).is_ok())
			.collect();
		identities.filter(DashboardIdentity::field_issuer().is_in(issuers))
	}

	/// Display Attributes are disclosed only for people who chose this Tenant:
	/// by addressing a Registration Request to it, through a Tenant Binding or
	/// an Invitation, or by creating it.
	fn view(&self, identity: &DashboardIdentity, chose: bool) -> TenantIdentity {
		TenantIdentity {
			id: identity.id,
			verified_email: chose.then(|| identity.verified_email.clone()).flatten(),
			display_name: chose.then(|| identity.display_name.clone()).flatten(),
		}
	}

	/// The External Identities among `ids` that chose this Tenant.
	async fn choosers(
		&self,
		tx: &mut AtomicTransaction,
		ids: &BTreeSet<Uuid>,
	) -> Result<BTreeSet<Uuid>> {
		if ids.is_empty() {
			return Ok(BTreeSet::new());
		}
		let mut chose: BTreeSet<Uuid> = DashboardRegistrationRequest::objects()
			.filter(DashboardRegistrationRequest::field_tenant().eq(Some(self.tenant().to_owned())))
			.filter(
				DashboardRegistrationRequest::field_identity_id()
					.is_in(ids.iter().copied().collect::<Vec<_>>()),
			)
			.all_with_db(tx)
			.await?
			.into_iter()
			.map(|row| row.identity_id())
			.collect();
		chose.extend(
			super::Tenant::objects()
				.filter(super::Tenant::field_name().eq(self.tenant().to_owned()))
				.filter(super::Tenant::field_created_by_kind().eq("identity".to_owned()))
				.all_with_db(tx)
				.await?
				.into_iter()
				.filter_map(|tenant| tenant.created_by_identity)
				.filter(|identity| ids.contains(identity)),
		);
		Ok(chose)
	}

	/// Refuses a change by this Tenant's own members that would leave no
	/// enabled Mapping whose subject `bundle` allows every administration
	/// action without conditions (ADR 0026). `leaving` is a Mapping the change
	/// disables. Deployment Administrators never pass through here.
	pub(crate) async fn require_remaining_administrator(
		&self,
		tx: &mut AtomicTransaction,
		bundle: &aidash_domain::policy::PolicyBundle,
		leaving: Option<Uuid>,
	) -> Result<()> {
		use aidash_domain::identity::tenancy::has_unconditional_administrator;
		let refused = || {
			Err(Error::Conflict(
				"the Tenant would be left without a Tenant Administrator".into(),
			))
		};
		// Which subjects could be the administrator follows from the bundle
		// alone, which is bounded; a subject the bundle does not list is
		// judged like any other unlisted one.
		let unlisted = format!("unlisted-{}", Uuid::new_v4().simple());
		let any_subject = !bundle.subjects.contains_key(&unlisted)
			&& has_unconditional_administrator(bundle, [unlisted.as_str()]);
		let listed: Vec<String> = bundle
			.subjects
			.keys()
			.filter(|subject| has_unconditional_administrator(bundle, [subject.as_str()]))
			.cloned()
			.collect();
		if !any_subject && listed.is_empty() {
			return refused();
		}
		// One bounded question, however many Mappings the Tenant has: does an
		// enabled Mapping of an enabled External Identity, whose credential
		// can still act, map such a subject?
		let now = super::dashboard_administration::database_time(tx).await?;
		let mapping = |column: &str| (Alias::new("m"), Alias::new(column));
		let mut query = Query::select();
		query
			.expr(Expr::value(1_i32))
			.from_as(Alias::new(DashboardMapping::table_name()), Alias::new("m"))
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(
					Alias::new(DashboardIdentity::table_name()),
					Alias::new("i"),
				),
				Expr::col((Alias::new("i"), Alias::new("id"))).equals(mapping("identity_id")),
			)
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(
					Alias::new(super::AuthorizationCredential::table_name()),
					Alias::new("c"),
				),
				Expr::col((Alias::new("c"), Alias::new("id"))).equals(mapping("credential_id")),
			)
			.and_where(Expr::col(mapping("tenant")).eq(Expr::value(self.tenant().to_owned())))
			.and_where(Expr::col(mapping("enabled")).eq(Expr::value(true)))
			.and_where(Expr::col((Alias::new("i"), Alias::new("disabled_at"))).is_null())
			// A Mapping whose credential is revoked or expired cannot act.
			.and_where(Expr::col((Alias::new("c"), Alias::new("revoked_at"))).is_null())
			.and_where(Expr::col((Alias::new("c"), Alias::new("expires_at"))).gt(Expr::value(now)))
			.limit(1);
		if let Some(leaving) = leaving {
			query.and_where(Expr::col(mapping("id")).ne(Expr::value(leaving)));
		}
		if !any_subject {
			query.and_where(
				Expr::col(mapping("subject")).is_in(listed.into_iter().map(Expr::value)),
			);
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		if TransactionExecutor::fetch_optional(tx, &sql, convert_values(values))
			.await?
			.is_some()
		{
			Ok(())
		} else {
			refused()
		}
	}

	pub(crate) fn policy(&self) -> &AccountPolicy {
		&self.policy
	}

	/// Locks the policy, then this administrator's credential, Mapping,
	/// External Identity and session, matching every execution boundary.
	pub(crate) async fn lock(
		&self,
		tx: &mut dyn TransactionExecutor,
		exclusive: bool,
	) -> Result<Snapshot> {
		self.identity
			.lock_native(tx, exclusive, Some(&self.policy))
			.await
	}

	/// [`Self::lock`] for restoring this administrator's deleted Tenant.
	pub(crate) async fn lock_deleted(&self, tx: &mut dyn TransactionExecutor) -> Result<Snapshot> {
		self.identity.lock_deleted(tx, Some(&self.policy)).await
	}

	/// The issuer whose External Identities are restricted to this Tenant.
	pub(crate) fn restricted_issuer(&self) -> Option<&str> {
		self.restricted.as_deref()
	}

	fn evaluation(&self, action: &str, id: &str, attributes: Value) -> Evaluation {
		Evaluation {
			subject: self.identity.subject.clone(),
			action: action.into(),
			resource: Resource {
				tenant: self.identity.tenant.clone(),
				kind: aidash_domain::identity::tenant_administration::resource_kind(action).into(),
				id: id.into(),
				attributes,
			},
			environment: json!({"node_id": self.node_id, "transport": "api"}),
		}
	}

	/// Evaluates and audits one action without failing on a denial.
	pub(crate) async fn allowed(
		&self,
		tx: &mut dyn TransactionExecutor,
		snapshot: &Snapshot,
		action: &str,
		id: &str,
		attributes: Value,
	) -> Result<bool> {
		let evaluation = self.evaluation(action, id, attributes);
		let mut decision = snapshot.bundle.evaluate(&evaluation);
		decision.revision = snapshot.revision;
		AuthorizationDecision::append(tx, self.tenant(), &evaluation, &decision).await?;
		Ok(decision.allowed)
	}

	/// Requires one action. A denial is kept for `record_denial`, because the
	/// caller's transaction, and any decision written to it, rolls back.
	pub(crate) async fn require(
		&self,
		tx: &mut dyn TransactionExecutor,
		snapshot: &Snapshot,
		action: &str,
		id: &str,
		attributes: Value,
	) -> Result<()> {
		let (evaluation, decision) = self.authorize(snapshot, action, id, attributes)?;
		AuthorizationDecision::append(tx, self.tenant(), &evaluation, &decision).await
	}

	/// Checks under the caller's retained policy lock. The caller must append
	/// the returned decision before committing a successful operation. This
	/// lets long reads acquire the global audit cursor lock only at the end.
	pub(crate) fn authorize(
		&self,
		snapshot: &Snapshot,
		action: &str,
		id: &str,
		attributes: Value,
	) -> Result<(Evaluation, Decision)> {
		let evaluation = self.evaluation(action, id, attributes);
		let mut decision = snapshot.bundle.evaluate(&evaluation);
		decision.revision = snapshot.revision;
		if decision.allowed {
			return Ok((evaluation, decision));
		}
		*self.denial.lock().expect("denial lock") = Some((evaluation, decision));
		Err(Error::Forbidden)
	}

	/// Commits a denial recorded by `require` after its transaction rolled back.
	pub(crate) async fn record_denial(&self, pool: &crate::database::native::Pool) -> Result<()> {
		let Some((evaluation, decision)) = self.denial.lock().expect("denial lock").take() else {
			return Ok(());
		};
		let mut tx = pool.begin(&self.scope()?).await?.into_executor().await?;
		AuthorizationDecision::append(tx.as_mut(), self.tenant(), &evaluation, &decision).await?;
		tx.commit().await?;
		Ok(())
	}

	/// Writes a changed policy as this administrator's revision.
	pub(crate) async fn save_policy(
		&self,
		tx: &mut dyn TransactionExecutor,
		snapshot: &Snapshot,
	) -> Result<i64> {
		aidash_application::authorization::validate_replacement(
			self.tenant(),
			snapshot.revision,
			&snapshot.bundle,
			&self.revision_actor(),
		)?;
		super::AuthorizationBundle::replace(
			tx,
			self.tenant(),
			snapshot.revision,
			serde_json::to_value(&snapshot.bundle)?,
			&self.revision_actor(),
			Some(self.mapping_id),
		)
		.await
	}

	fn revision_actor(&self) -> String {
		format!("oidc:{}", self.identity_id)
	}
}

impl TenantAdministrator {
	/// The Tenant Administrator and Registry actions this caller holds.
	/// Ordinary members receive no administration actions and no policy details.
	pub(crate) async fn overview(&self, db: &Pool) -> Result<TenantAdministration> {
		db.atomic(&self.scope()?, async |tx| {
			let snapshot = self.lock(tx, false).await?;
			let mut actions = vec![];
			for action in rules::ACTIONS {
				if self
					.allowed(tx, &snapshot, action, rules::COLLECTION, json!({}))
					.await?
				{
					actions.push(action.to_owned());
				}
			}
			let administrator = !actions.is_empty();
			for action in rules::REGISTRY_ACTIONS {
				if self
					.allowed(tx, &snapshot, action, rules::COLLECTION, json!({}))
					.await?
				{
					actions.push(action.to_owned());
				}
			}
			let tenant = self.tenant().to_owned();
			for action in [
				aidash_domain::identity::tenancy::TENANT_UPDATE,
				aidash_domain::identity::tenancy::TENANT_DELETE,
			] {
				if self
					.allowed(tx, &snapshot, action, &tenant, json!({}))
					.await?
				{
					actions.push(action.to_owned());
				}
			}
			let own = self.identity.subject.clone();
			if self
				.allowed(tx, &snapshot, rules::CREDENTIAL_CREATE, &own, json!({}))
				.await?
			{
				actions.push(rules::CREDENTIAL_CREATE.to_owned());
			}
			let candidates: Vec<_> = snapshot
				.bundle
				.subjects
				.iter()
				.filter(|(id, subject)| {
					aidash_domain::identity::authority::enabled(&snapshot.bundle, id)
						&& matches!(
							subject.kind,
							aidash_domain::policy::SubjectKind::Service
								| aidash_domain::policy::SubjectKind::Agent
						)
				})
				.collect();
			let mut mapped = BTreeSet::<String>::new();
			if !candidates.is_empty() {
				let (sql, values) = Query::select()
					.distinct()
					.column(Alias::new("subject"))
					.from(Alias::new(DashboardMapping::table_name()))
					.and_where(Expr::col("tenant").eq(Expr::value(self.tenant())))
					.and_where(
						Expr::col("subject")
							.is_in(candidates.iter().map(|(id, _)| Expr::value(id.as_str()))),
					)
					.build(PostgresQueryBuilder);
				for row in TransactionExecutor::fetch_all(tx, &sql, convert_values(values)).await? {
					mapped.insert(
						row.get("subject")
							.map_err(reinhardt::core::exception::Error::from)?,
					);
				}
			}
			let mut issuable_subjects = vec![];
			let mut decisions = vec![];
			for (id, subject) in candidates {
				if mapped.contains(id) {
					continue;
				}
				let evaluation =
					self.evaluation(rules::CREDENTIAL_ISSUE, id, json!({"kind": subject.kind}));
				let mut decision = snapshot.bundle.evaluate(&evaluation);
				decision.revision = snapshot.revision;
				if decision.allowed {
					issuable_subjects.push(id.clone());
				}
				decisions.push((evaluation, decision));
			}
			AuthorizationDecision::append_many(tx, self.tenant(), &decisions).await?;
			let label = super::Tenant::objects()
				.filter(super::Tenant::field_name().eq(tenant))
				.all_with_db(tx)
				.await?
				.pop()
				.and_then(|root| root.label);
			Ok(TenantAdministration {
				label,
				tenant: self.tenant().to_owned(),
				actions,
				issuable_subjects,
				assignable_groups: if administrator {
					rules::assignable_groups(&snapshot.bundle)
				} else {
					Default::default()
				},
				policy_revision: administrator.then_some(snapshot.revision),
			})
		})
		.await
	}

	/// One page of pending Registration Requests addressed to this Tenant,
	/// through a Tenant Binding or an Invitation, other than this
	/// administrator's own, newest first. Requests addressed elsewhere or to no
	/// Tenant, and those of hidden identities, are excluded in SQL before
	/// paging, so a large bound pool never loads its whole queue.
	pub(crate) async fn registrations(
		&self,
		db: &Pool,
		offset: u64,
	) -> Result<Vec<TenantRegistration>> {
		db.atomic(&self.scope()?, async |tx| {
			let snapshot = self.lock(tx, false).await?;
			self.require(
				tx,
				&snapshot,
				rules::REGISTRATION_READ,
				rules::COLLECTION,
				json!({}),
			)
			.await?;
			let now = super::dashboard_administration::database_time(tx).await?;
			let pending = DashboardRegistrationRequest::objects()
				.filter(
					DashboardRegistrationRequest::field_tenant().eq(Some(self.tenant().to_owned())),
				)
				.filter(DashboardRegistrationRequest::field_status().eq("pending"))
				.filter(DashboardRegistrationRequest::field_expires_at().gt(now))
				.filter(DashboardRegistrationRequest::field_identity_id().ne(self.identity_id))
				.filter_in_subquery("identity_id", |identities| {
					self.visible(identities).values(&["id"])
				})?
				.order_by(&["-created_at", "-id"])
				.limit(200)
				.offset(offset as usize)
				.all_with_db(tx)
				.await?;
			let identities = self
				.identities(tx, pending.iter().map(|row| row.identity_id()).collect())
				.await?;
			let invitations: BTreeMap<Uuid, super::TenantInvitation> =
				super::TenantInvitation::objects()
					.filter(
						super::TenantInvitation::field_id().is_in(
							pending
								.iter()
								.filter_map(|row| row.invitation_id)
								.collect::<Vec<_>>(),
						),
					)
					.all_with_db(tx)
					.await?
					.into_iter()
					.map(|invitation| (invitation.id, invitation))
					.collect();
			Ok(pending
				.into_iter()
				.filter_map(|row| {
					let identity = identities.get(&row.identity_id())?;
					Some(TenantRegistration {
						id: row.id,
						identity: self.view(identity, true),
						created_at: row.created_at,
						expires_at: row.expires_at,
						invitation_id: row.invitation_id,
						proposed_groups: row
							.invitation_id
							.and_then(|id| invitations.get(&id))
							.map(crate::apps::identity::repositories::invitations::proposed)
							.unwrap_or_default(),
					})
				})
				.collect())
		})
		.await
	}

	/// This Tenant's Mappings with each subject's Assignable Group memberships.
	/// Mappings of hidden identities are excluded before paging.
	pub(crate) async fn mappings(&self, db: &Pool, offset: u64) -> Result<Vec<TenantMapping>> {
		db.atomic(&self.scope()?, async |tx| {
			let snapshot = self.lock(tx, false).await?;
			self.require(
				tx,
				&snapshot,
				rules::MAPPING_READ,
				rules::COLLECTION,
				json!({}),
			)
			.await?;
			let mappings = DashboardMapping::objects()
				.filter(DashboardMapping::field_tenant().eq(self.tenant().to_owned()))
				.filter_in_subquery("identity_id", |identities| {
					self.visible(identities).values(&["id"])
				})?
				.order_by(&["subject", "id"])
				.limit(200)
				.offset(offset as usize)
				.all_with_db(tx)
				.await?;
			let identities = self
				.identities(tx, mappings.iter().map(|row| row.identity_id()).collect())
				.await?;
			let chose = self
				.choosers(tx, &identities.keys().copied().collect())
				.await?;
			let own_subjects = self.own_subjects(tx).await?;
			Ok(mappings
				.into_iter()
				.filter_map(|row| {
					let identity = identities.get(&row.identity_id())?;
					Some(TenantMapping {
						id: row.id,
						identity: self.view(identity, chose.contains(&identity.id)),
						groups: rules::memberships(&snapshot.bundle, &row.subject),
						own_identity: identity.id == self.identity_id,
						own_subject: own_subjects.contains(&row.subject),
						subject: row.subject,
						enabled: row.enabled,
						revision: row.revision,
					})
				})
				.collect())
		})
		.await
	}

	/// The subject this administrator acts as and every subject its External
	/// Identity was mapped to in this Tenant, including through disabled
	/// Mappings. It never changes their groups.
	async fn own_subjects(&self, tx: &mut AtomicTransaction) -> Result<BTreeSet<String>> {
		let mut subjects: BTreeSet<String> = DashboardMapping::objects()
			.filter(DashboardMapping::field_identity_id().eq(self.identity_id))
			.filter(DashboardMapping::field_tenant().eq(self.tenant().to_owned()))
			.all_with_db(tx)
			.await?
			.into_iter()
			.map(|row| row.subject)
			.collect();
		subjects.insert(self.identity.subject.clone());
		Ok(subjects)
	}

	/// Only External Identities this administrator `sees` are ever disclosed.
	async fn identities(
		&self,
		tx: &mut AtomicTransaction,
		ids: Vec<Uuid>,
	) -> Result<BTreeMap<Uuid, DashboardIdentity>> {
		if ids.is_empty() {
			return Ok(BTreeMap::new());
		}
		Ok(self
			.visible(DashboardIdentity::objects().filter(DashboardIdentity::field_id().is_in(ids)))
			.all_with_db(tx)
			.await?
			.into_iter()
			.map(|identity| (identity.id, identity))
			.collect())
	}

	/// Replaces a user subject's Assignable Group memberships, never its own.
	/// `actor` is this administrator, which records a change in the
	/// administration history.
	pub(crate) async fn update_memberships(
		&self,
		db: &Pool,
		actor: &AdministrationActor,
		subject: String,
		input: MembershipUpdate,
	) -> Result<Membership> {
		aidash_domain::policy::identifier(&subject)?;
		db.atomic(&self.scope()?, async |tx| {
			let mut snapshot = self.lock(tx, true).await?;
			if snapshot.revision != input.expected_policy_revision {
				return Err(Error::Conflict("authorization revision changed".into()));
			}
			if self.own_subjects(tx).await?.contains(&subject) {
				return Err(Error::Forbidden);
			}
			self.require(
				tx,
				&snapshot,
				rules::SUBJECT_GROUP_UPDATE,
				&subject,
				json!({"groups": input.groups}),
			)
			.await?;
			if rules::replace_assignable_memberships(&mut snapshot.bundle, &subject, &input.groups)?
			{
				self.require_remaining_administrator(tx, &snapshot.bundle, None)
					.await?;
				snapshot.revision = self.save_policy(tx, &snapshot).await?;
				actor
					.record(
						tx,
						AdministrationEvent {
							action: rules::SUBJECT_GROUP_UPDATE,
							identity: None,
							registration: None,
							mapping: None,
							tenant: Some(self.tenant()),
							subject: Some(&subject),
							invitation: None,
						},
					)
					.await?;
			}
			Ok(Membership {
				groups: rules::memberships(&snapshot.bundle, &subject),
				subject,
				policy_revision: snapshot.revision,
			})
		})
		.await
	}
}

mod credentials;
mod peer_mappings;
mod policy;
