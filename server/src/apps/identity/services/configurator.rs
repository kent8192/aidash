//! Authority held only while the server dispatches a local Configurator call.
use crate::apps::identity::models::DashboardExecutionOrigin;
use crate::{Error, Result, authorization::identity::SubjectIdentity, federation::Federation};
use aidash_application::registry::system::configurator::{ID, VERSION};
use aidash_domain::{Run, policy::PolicyBundle};
use reinhardt::db::orm::Model;
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct Caller {
	pub(super) identity: SubjectIdentity,
	identity_id: Uuid,
	mapping: Uuid,
	executor: String,
	deployment: String,
	pub thread: Uuid,
	pub run: Uuid,
}

tokio::task_local! {static CALLER: Caller;}

impl Caller {
	pub(crate) async fn from_run(f: &Federation, run: &Run) -> Result<Self> {
		if run.agent_id != ID
			|| run.agent_version != VERSION
			|| !run.is_local(f.config.deployment.as_str())
		{
			return Err(Error::Forbidden);
		}
		let grant = aidash_application::authorization::execution::grant(
			&crate::bootstrap::execution_grants(&f.store),
			&run.into(),
		)
		.await?
		.ok_or(Error::Forbidden)?;
		let executor = aidash_domain::agent_subject(&run.home_node, ID, VERSION);
		if grant.subject_chain.as_slice() != [grant.root_subject.as_str(), executor.as_str()] {
			return Err(Error::Forbidden);
		}
		let identity = SubjectIdentity {
			http_session: None,
			credential_id: grant.credential_id,
			tenant: grant.tenant,
			subject: grant.root_subject,
		};
		identity.check_binding(&f.store.pool).await?;
		let (identity_id, mapping) = f
			.store
			.pool
			.atomic(&identity.tenancy()?, async |tx| {
				DashboardExecutionOrigin::require_current(
					tx,
					run.id,
					f.config.dashboard_policy().as_ref(),
					aidash_application::ports::accounts::Recheck::Online,
				)
				.await?;
				let snapshot = identity
					.lock_native(tx, false, f.config.dashboard_policy().as_ref())
					.await?;
				if !aidash_domain::identity::tenant_administration::configurator_administrator(
					&snapshot.bundle,
					&identity.subject,
					f.config.deployment.as_str(),
				) {
					return Err(Error::Forbidden);
				}
				super::identity::require_live_subject(tx, &snapshot, &executor).await?;
				let origin = DashboardExecutionOrigin::objects()
					.filter(DashboardExecutionOrigin::field_run_id().eq(run.id))
					.filter(DashboardExecutionOrigin::field_tenant().eq(identity.tenant.clone()))
					.all_with_executor(tx)
					.await
					.map_err(reinhardt::core::exception::Error::from)?
					.pop()
					.ok_or(Error::Forbidden)?;
				Ok((origin.identity_id(), origin.mapping_id()))
			})
			.await?;
		Ok(Self {
			identity,
			identity_id,
			mapping,
			executor,
			deployment: f.config.deployment.to_string(),
			thread: run.thread_id,
			run: run.id,
		})
	}

	pub(crate) fn actor(&self) -> crate::authorization::identity::Actor {
		crate::authorization::identity::Actor::Subject(self.identity.clone())
	}
	pub(crate) fn subjects(&self) -> Vec<String> {
		vec![self.identity.subject.clone(), self.executor.clone()]
	}
	pub(crate) fn tenant(&self) -> &str {
		&self.identity.tenant
	}
	pub(crate) async fn during<F: std::future::Future>(&self, f: F) -> F::Output {
		CALLER.scope(self.clone(), f).await
	}
}

pub(crate) fn current(identity: &SubjectIdentity) -> Option<Caller> {
	CALLER
		.try_with(|caller| {
			(caller.identity.credential_id == identity.credential_id
				&& caller.identity.subject == identity.subject
				&& caller.identity.tenant == identity.tenant)
				.then(|| caller.clone())
		})
		.ok()
		.flatten()
}

pub(super) fn mapping(
	identity: &SubjectIdentity,
	origin: Option<&super::oidc::BrowserOrigin>,
) -> Result<(Uuid, Uuid)> {
	if let Some(origin) = origin {
		return Ok((
			origin.identity_id,
			origin.mapping_id.ok_or(Error::Forbidden)?,
		));
	}
	current(identity)
		.map(|c| (c.identity_id, c.mapping))
		.ok_or(Error::Forbidden)
}

/// Apply the same chain to policy decisions inside browser-oriented services.
/// This context is request-local and cannot be written into a policy revision.
pub(crate) fn constrain(bundle: &mut PolicyBundle) -> Result<()> {
	CALLER
		.try_with(|caller| {
			if bundle.tenant != caller.identity.tenant
				|| !aidash_domain::identity::tenant_administration::configurator_administrator(
					bundle,
					&caller.identity.subject,
					&caller.deployment,
				) || !aidash_domain::identity::authority::enabled(bundle, &caller.executor)
			{
				return Err(Error::Forbidden);
			}
			bundle.evaluation_chain =
				Some((caller.identity.subject.clone(), caller.executor.clone()));
			Ok(())
		})
		.unwrap_or(Ok(()))
}
