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
use reinhardt::db::orm::{AtomicTransaction, DatabaseConnection, Model};
use serde_json::{Value, json};
use std::collections::BTreeMap;
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
}

/// The fields of one administration history row other than its actor.
pub(crate) struct AdministrationEvent<'a> {
	pub action: &'a str,
	pub identity: Uuid,
	pub registration: Option<Uuid>,
	pub mapping: Option<Uuid>,
	pub tenant: Option<&'a str>,
	pub subject: Option<&'a str>,
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
		}
	}

	pub(crate) fn tenant_administrator(&self) -> Option<&TenantAdministrator> {
		match self {
			Self::TenantAdministrator(administrator) => Some(administrator),
			_ => None,
		}
	}

	pub(crate) async fn record(
		&self,
		tx: &mut AtomicTransaction,
		event: AdministrationEvent<'_>,
	) -> Result<()> {
		let (kind, identity, mapping) = match self {
			Self::OperatorBearer => ("operator", None, None),
			Self::Operator { identity } => ("operator", Some(*identity), None),
			Self::TenantAdministrator(administrator) => (
				"tenant_administrator",
				Some(administrator.identity_id),
				Some(administrator.mapping_id),
			),
		};
		let row = DashboardAdministrationHistory::build()
			.id(Uuid::new_v4())
			.occurred_at(super::dashboard_administration::database_time(tx).await?)
			.action(event.action.to_owned())
			.identity_id(event.identity)
			.registration_id(event.registration)
			.mapping_id(event.mapping)
			.tenant(event.tenant.map(str::to_owned))
			.subject(event.subject.map(str::to_owned))
			.actor_kind(kind.to_owned())
			.actor_identity_id(identity)
			.actor_mapping_id(mapping)
			.finish();
		DashboardAdministrationHistory::objects()
			.create_with_conn(tx, &row)
			.await?;
		Ok(())
	}
}

/// An External Identity acting through one of its Mappings. It exists only on
/// deployments whose Tenant Binding ties Registration Requests to its Tenant.
pub(crate) struct TenantAdministrator {
	pub identity: SubjectIdentity,
	pub identity_id: Uuid,
	pub mapping_id: Uuid,
	node_id: String,
	pool: String,
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
		let pool = policy
			.bound_pool(&identity.tenant)
			.ok_or(Error::Forbidden)?
			.to_owned();
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

	/// The GCIP Tenant whose External Identities this Tenant may approve.
	pub(crate) fn pool(&self) -> &str {
		&self.pool
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
			Ok(TenantAdministration {
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

	/// Pending Registration Requests from the bound GCIP Tenant only.
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
				.filter(DashboardRegistrationRequest::field_status().eq("pending"))
				.filter(DashboardRegistrationRequest::field_expires_at().gt(now))
				.order_by(&["-created_at", "-id"])
				.all_with_db(tx)
				.await?;
			let identities = self
				.identities(tx, pending.iter().map(|row| row.identity_id()).collect())
				.await?;
			Ok(pending
				.into_iter()
				.filter_map(|row| {
					let identity = identities.get(&row.identity_id())?;
					(identity.id != self.identity_id).then(|| TenantRegistration {
						id: row.id,
						identity: view(identity),
						created_at: row.created_at,
						expires_at: row.expires_at,
					})
				})
				.collect())
		})
		.await
	}

	/// This Tenant's Mappings with each subject's Assignable Group memberships.
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
				.order_by(&["subject", "id"])
				.limit(200)
				.offset(offset as usize)
				.all_with_db(tx)
				.await?;
			let identities = self
				.identities(tx, mappings.iter().map(|row| row.identity_id()).collect())
				.await?;
			Ok(mappings
				.into_iter()
				.filter_map(|row| {
					let identity = identities.get(&row.identity_id())?;
					Some(TenantMapping {
						id: row.id,
						identity: view(identity),
						groups: rules::memberships(&snapshot.bundle, &row.subject),
						subject: row.subject,
						enabled: row.enabled,
						revision: row.revision,
					})
				})
				.collect())
		})
		.await
	}

	/// Only External Identities of the bound GCIP Tenant are ever disclosed.
	async fn identities(
		&self,
		tx: &mut AtomicTransaction,
		ids: Vec<Uuid>,
	) -> Result<BTreeMap<Uuid, DashboardIdentity>> {
		if ids.is_empty() {
			return Ok(BTreeMap::new());
		}
		Ok(DashboardIdentity::objects()
			.filter(DashboardIdentity::field_id().is_in(ids))
			.filter(DashboardIdentity::field_gcip_tenant().eq(self.pool().to_owned()))
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
			let own = subject == self.identity.subject
				|| DashboardMapping::objects()
					.filter(DashboardMapping::field_identity_id().eq(self.identity_id))
					.filter(DashboardMapping::field_tenant().eq(self.tenant().to_owned()))
					.filter(DashboardMapping::field_subject().eq(subject.clone()))
					.exists_with_db(tx)
					.await?;
			if own {
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

fn view(identity: &DashboardIdentity) -> TenantIdentity {
	TenantIdentity {
		id: identity.id,
		verified_email: identity.verified_email.clone(),
		display_name: identity.display_name.clone(),
	}
}
