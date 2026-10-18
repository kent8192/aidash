//! Tenant Administrator use cases. Only an External Identity acting through a
//! Mapping can be a Tenant Administrator; it needs no Tenant Binding.
use crate::apps::identity::models::{
	AdministrationActor, DashboardMapping, DashboardRegistrationRequest, TenantAdministrator,
};
use crate::apps::identity::oidc::{BrowserOrigin, digest, random_secret};
use crate::apps::identity::serializers::oidc::{
	Approval, ApprovedMapping, MappingRevision, Registration,
};
use crate::apps::identity::serializers::tenant_administration::{
	Membership, MembershipUpdate, TenantAdministration, TenantApproval, TenantMapping,
	TenantRegistration,
};
use crate::authorization::identity::Actor;
use crate::federation::Federation;
use crate::http::validate;
use crate::{Error, Result};
use reinhardt::injectable;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct TenantAdministrations {
	pub runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> TenantAdministrations {
	TenantAdministrations { runtime }
}

impl TenantAdministrations {
	/// Bearer Subject Credentials and Deployment Administrators are never Tenant Administrators.
	async fn administrator(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: &str,
	) -> Result<Arc<TenantAdministrator>> {
		aidash_domain::policy::identifier(tenant)?;
		let (Actor::Subject(identity), Some(origin)) = (actor, origin) else {
			return Err(Error::Forbidden);
		};
		let mapping = origin.mapping_id.ok_or(Error::Forbidden)?;
		if identity.tenant != tenant {
			return Err(Error::Forbidden);
		}
		// Persist a removed Tenant Binding's disablement before any lease.
		identity.check_binding(&self.runtime.store.pool).await?;
		Ok(Arc::new(TenantAdministrator::new(
			identity,
			origin.identity_id,
			mapping,
			self.runtime.config.node_id.clone(),
			self.runtime.config.dashboard_policy(),
		)?))
	}

	/// A denial is audited after its rolled-back transaction.
	async fn finish<T>(&self, administrator: &TenantAdministrator, result: Result<T>) -> Result<T> {
		if matches!(result, Err(Error::Forbidden)) {
			administrator
				.record_denial(&self.runtime.store.pool)
				.await?;
		}
		result
	}

	pub(crate) async fn overview(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
	) -> Result<TenantAdministration> {
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let mut overview = administrator.overview(&self.runtime.store.pool).await?;
		// With Invitations turned off for the Deployment nobody may issue one,
		// whatever the policy allows, so the page offers no issue form;
		// existing links stay listed and revocable.
		if !self.runtime.config.tenancy.invitations {
			overview.actions.retain(|action| {
				action != aidash_domain::identity::tenant_administration::INVITATION_CREATE
			});
		}
		Ok(overview)
	}

	pub(crate) async fn registrations(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		offset: u64,
	) -> Result<Vec<TenantRegistration>> {
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let result = administrator
			.registrations(&self.runtime.store.pool, offset)
			.await;
		self.finish(&administrator, result).await
	}

	pub(crate) async fn approve(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		id: Uuid,
		input: TenantApproval,
	) -> Result<ApprovedMapping> {
		validate(&input)?;
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let policy = Some(administrator.policy().clone());
		let actor = AdministrationActor::TenantAdministrator(administrator.clone());
		let approve = || {
			DashboardRegistrationRequest::approve(
				&self.runtime.store.pool,
				id,
				Approval {
					tenant: tenant.clone(),
					subject: input.subject.clone(),
					groups: None,
				},
				input.groups.clone(),
				&actor,
				digest(&random_secret()),
				policy.clone(),
			)
		};
		let mut result = approve().await;
		// A previous owner's connection must be gone before the subject is
		// mapped again; approval refuses while one remains (ADR 0007). It is
		// retired only for an approval refused for that alone, so an invalid
		// or stale decision changes nothing, and the approval is then retried.
		if result.as_ref().is_err_and(
			crate::apps::identity::repositories::plan_connections::refused_shared_holder,
		) {
			super::plan_connections::retire_unmapped(
				&self.runtime,
				&tenant,
				&input.subject,
				&actor.decision_actor(),
			)
			.await?;
			result = approve().await;
		}
		self.finish(&administrator, result).await
	}

	pub(crate) async fn reject(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		id: Uuid,
	) -> Result<Registration> {
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let idle_seconds = self
			.runtime
			.config
			.dashboard_session()
			.map(|config| config.session_idle_seconds);
		let result = DashboardRegistrationRequest::reject(
			&self.runtime.store.pool,
			id,
			&AdministrationActor::TenantAdministrator(administrator.clone()),
			idle_seconds,
		)
		.await;
		self.finish(&administrator, result).await
	}

	pub(crate) async fn mappings(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		offset: u64,
	) -> Result<Vec<TenantMapping>> {
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let result = administrator
			.mappings(&self.runtime.store.pool, offset)
			.await;
		self.finish(&administrator, result).await
	}

	pub(crate) async fn disable_mapping(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		id: Uuid,
		input: MappingRevision,
	) -> Result<http::StatusCode> {
		validate(&input)?;
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let actor = AdministrationActor::TenantAdministrator(administrator.clone());
		let result = DashboardMapping::disable(
			&self.runtime.store.pool,
			id,
			input.expected_revision,
			&actor,
		)
		.await;
		self.finish(&administrator, result).await?;
		super::plan_connections::retire_disabled_mapping(
			&self.runtime,
			id,
			&actor.decision_actor(),
		)
		.await;
		Ok(http::StatusCode::NO_CONTENT)
	}

	pub(crate) async fn update_memberships(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		subject: String,
		input: MembershipUpdate,
	) -> Result<Membership> {
		validate(&input)?;
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let result = administrator
			.update_memberships(
				&self.runtime.store.pool,
				&AdministrationActor::TenantAdministrator(administrator.clone()),
				subject,
				input,
			)
			.await;
		self.finish(&administrator, result).await
	}
}
