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
use crate::{Error, Result};
use aidash_application::ports::authorization::dashboard::AccountPolicy;
use aidash_domain::identity::tenant_administration as rules;
use aidash_domain::policy::{Decision, Evaluation, Resource};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{AtomicTransaction, DatabaseConnection, Model, QuerySet};
use reinhardt::query::{Alias, IntoValue, PostgresQueryBuilder, Query, QueryStatementBuilder};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Who decided a Registration Request or disabled a Mapping.
#[derive(Clone)]
pub(crate) enum AdministrationActor {
	/// The deployment's operator bearer token.
	OperatorBearer,
	/// A browser session holding an enabled operator grant.
	Operator {
		identity: Uuid,
	},
	TenantAdministrator(Arc<TenantAdministrator>),
	/// An External Identity acting for itself, such as creating a Tenant,
	/// redeeming an Invitation or relabeling its Tenant through a Mapping.
	Identity {
		identity: Uuid,
		mapping: Option<Uuid>,
	},
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
			Self::OperatorBearer => "operator-bearer".into(),
			Self::Operator { identity } => format!("oidc:{identity}"),
			Self::TenantAdministrator(administrator) => {
				format!("oidc:{}", administrator.identity_id)
			}
			Self::Identity { identity, .. } => format!("oidc:{identity}"),
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
			Self::OperatorBearer => ("operator", None, None),
			Self::Operator { identity } => ("operator", Some(*identity), None),
			Self::TenantAdministrator(administrator) => (
				"tenant_administrator",
				Some(administrator.identity_id),
				Some(administrator.mapping_id),
			),
			Self::Identity { identity, mapping } => ("identity", Some(*identity), *mapping),
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
	/// The GCIP Tenant bound to this Tenant, if any.
	pool: Option<String>,
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
		let pool = policy.bound_pool(&identity.tenant).map(str::to_owned);
		Ok(Self {
			identity,
			identity_id,
			mapping_id,
			node_id,
			pool,
			policy,
			denial: Mutex::new(None),
		})
	}

	pub(crate) fn tenant(&self) -> &str {
		&self.identity.tenant
	}

	/// Whether this administrator may decide `registration` from `identity`: it
	/// is addressed to this Tenant, through a Tenant Binding or an Invitation,
	/// and comes from the configured issuer. Requests addressed to no Tenant
	/// are decided only by Deployment Administrators.
	pub(crate) fn sees(
		&self,
		registration: &DashboardRegistrationRequest,
		identity: &DashboardIdentity,
	) -> bool {
		registration.tenant.as_deref() == Some(self.tenant())
			&& identity.issuer == self.policy.issuer
			&& self
				.pool
				.as_deref()
				.is_none_or(|pool| identity.gcip_tenant.is_empty() || identity.gcip_tenant == pool)
	}

	/// Narrows `identities` to the configured issuer's and, under a Tenant
	/// Binding, to its bound GCIP Tenant, in SQL. Without a Binding, callers
	/// pass only identities already mapped into this Tenant.
	fn visible(&self, identities: QuerySet<DashboardIdentity>) -> QuerySet<DashboardIdentity> {
		let identities =
			identities.filter(DashboardIdentity::field_issuer().eq(self.policy.issuer.clone()));
		match &self.pool {
			Some(pool) => {
				identities.filter(DashboardIdentity::field_gcip_tenant().eq(pool.clone()))
			}
			None => identities,
		}
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
		let mappings: Vec<_> = DashboardMapping::objects()
			.filter(DashboardMapping::field_tenant().eq(self.tenant().to_owned()))
			.filter(DashboardMapping::field_enabled().eq(true))
			.all_with_db(tx)
			.await?
			.into_iter()
			.filter(|mapping| Some(mapping.id) != leaving)
			.collect();
		let enabled: BTreeSet<Uuid> = DashboardIdentity::objects()
			.filter(
				DashboardIdentity::field_id().is_in(
					mappings
						.iter()
						.map(|mapping| mapping.identity_id())
						.collect::<Vec<_>>(),
				),
			)
			.filter(DashboardIdentity::field_disabled_at().is_null())
			.all_with_db(tx)
			.await?
			.into_iter()
			.map(|identity| identity.id)
			.collect();
		// A Mapping whose credential is revoked or expired cannot act.
		let now = super::dashboard_administration::database_time(tx).await?;
		let usable: BTreeSet<Uuid> = super::AuthorizationCredential::objects()
			.filter(
				super::AuthorizationCredential::field_id().is_in(
					mappings
						.iter()
						.map(|mapping| mapping.credential_id())
						.collect::<Vec<_>>(),
				),
			)
			.filter(super::AuthorizationCredential::field_revoked_at().is_null())
			.filter(super::AuthorizationCredential::field_expires_at().gt(now))
			.all_with_db(tx)
			.await?
			.into_iter()
			.map(|credential| credential.id)
			.collect();
		let subjects = mappings
			.iter()
			.filter(|mapping| {
				enabled.contains(&mapping.identity_id())
					&& usable.contains(&mapping.credential_id())
			})
			.map(|mapping| mapping.subject.as_str());
		if aidash_domain::identity::tenancy::has_unconditional_administrator(bundle, subjects) {
			Ok(())
		} else {
			Err(Error::Conflict(
				"the Tenant would be left without a Tenant Administrator".into(),
			))
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
		let evaluation = self.evaluation(action, id, attributes);
		let mut decision = snapshot.bundle.evaluate(&evaluation);
		decision.revision = snapshot.revision;
		if decision.allowed {
			AuthorizationDecision::append(tx, self.tenant(), &evaluation, &decision).await?;
			return Ok(());
		}
		*self.denial.lock().expect("denial lock") = Some((evaluation, decision));
		Err(Error::Forbidden)
	}

	/// Commits a denial recorded by `require` after its transaction rolled back.
	pub(crate) async fn record_denial(&self, pool: &crate::database::native::Pool) -> Result<()> {
		let Some((evaluation, decision)) = self.denial.lock().expect("denial lock").take() else {
			return Ok(());
		};
		let mut tx = crate::database::native::begin(pool)
			.await?
			.into_executor()?;
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
		)
		.await
	}

	fn revision_actor(&self) -> String {
		format!("oidc:{}", self.identity_id)
	}
}

impl TenantAdministrator {
	/// The Tenant Administrator actions this caller holds. Ordinary members
	/// receive no actions and no policy details.
	pub(crate) async fn overview(&self, db: DatabaseConnection) -> Result<TenantAdministration> {
		db.atomic(async |tx| {
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
			let tenant = self.tenant().to_owned();
			if self
				.allowed(
					tx,
					&snapshot,
					aidash_domain::identity::tenancy::TENANT_UPDATE,
					&tenant,
					json!({}),
				)
				.await?
			{
				actions.push(aidash_domain::identity::tenancy::TENANT_UPDATE.to_owned());
			}
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

	/// Pending Registration Requests addressed to this Tenant, through a Tenant
	/// Binding or an Invitation, other than this administrator's own. Requests
	/// addressed elsewhere or to no Tenant are excluded in SQL.
	pub(crate) async fn registrations(
		&self,
		db: DatabaseConnection,
	) -> Result<Vec<TenantRegistration>> {
		db.atomic(async |tx| {
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
	pub(crate) async fn mappings(
		&self,
		db: DatabaseConnection,
		offset: u64,
	) -> Result<Vec<TenantMapping>> {
		db.atomic(async |tx| {
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
	pub(crate) async fn update_memberships(
		&self,
		db: DatabaseConnection,
		subject: String,
		input: MembershipUpdate,
	) -> Result<Membership> {
		aidash_domain::policy::identifier(&subject)?;
		db.atomic(async |tx| {
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
