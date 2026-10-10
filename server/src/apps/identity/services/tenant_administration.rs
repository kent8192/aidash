//! Tenant Administrator use cases. Only an External Identity acting through a
//! Mapping, on a deployment with Tenant Bindings, can be a Tenant Administrator.
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
	/// Bearer Subject Credentials and Operators are never Tenant Administrators.
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
		let lease = self.runtime.store.orm_connection()?;
		administrator.overview(lease.handle()).await
	}

	pub(crate) async fn registrations(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
	) -> Result<Vec<TenantRegistration>> {
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let lease = self.runtime.store.orm_connection()?;
		let result = administrator.registrations(lease.handle()).await;
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
		let lease = self.runtime.store.orm_connection()?;
		let result = DashboardRegistrationRequest::approve(
			lease.handle(),
			id,
			Approval {
				tenant,
				subject: input.subject,
			},
			input.groups,
			&AdministrationActor::TenantAdministrator(administrator.clone()),
			digest(&random_secret()),
			policy,
		)
		.await;
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
		let lease = self.runtime.store.orm_connection()?;
		let result = DashboardRegistrationRequest::reject(
			lease.handle(),
			id,
			&AdministrationActor::TenantAdministrator(administrator.clone()),
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
		let lease = self.runtime.store.orm_connection()?;
		let result = administrator.mappings(lease.handle(), offset).await;
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
		let lease = self.runtime.store.orm_connection()?;
		let result = DashboardMapping::disable(
			lease.handle(),
			id,
			input.expected_revision,
			&AdministrationActor::TenantAdministrator(administrator.clone()),
		)
		.await;
		self.finish(&administrator, result)
			.await
			.map(|()| http::StatusCode::NO_CONTENT)
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
		let lease = self.runtime.store.orm_connection()?;
		let result = administrator
			.update_memberships(lease.handle(), subject, input)
			.await;
		self.finish(&administrator, result).await
	}
}
