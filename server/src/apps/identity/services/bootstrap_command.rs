//! Host Access bootstrap of a Tenant and its administrators.
use crate::apps::identity::models::{
	AuthorizationBundle, AuthorizationRevision, BootstrapDeploymentAdministratorReservation,
	DashboardMapping, DemoReservation, Tenant, demo_reservations,
};
use crate::apps::identity::repositories::tenants::{self, Creator};
use crate::config::Config;
use crate::database::native::Pool;
use crate::{Error, Result};
use aidash_application::ports::authorization::dashboard::AccountPolicy;
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::identity::tenancy::{self, BootstrapSubject};
use aidash_domain::identity::tenant_administration;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, query::FieldAssignment};
use reinhardt::query::{
	Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use uuid::Uuid;

/// The issuer, source and actor name of everything Host Access does here.
const HOST_ACCESS: &str = "host_access";

#[derive(Debug)]
pub struct Outcome {
	pub tenant: String,
	pub created: bool,
	pub tenant_administrator: &'static str,
	/// The subject the address is reserved for or mapped to: `owner`, or a
	/// `host-administrator` subject when `owner` is unusable (#966).
	pub subject: String,
	pub deployment_administrator: &'static str,
}

/// Bootstraps `selector`'s Tenant for `email` and grants `email` Deployment
/// Administrator. The grant is Deployment-wide, so it is made even when the
/// Tenant cannot be recovered (#966); the Tenant's error is then returned with
/// the grant's outcome appended.
pub async fn bootstrap(
	pool: &Pool,
	config: &Config,
	email: &str,
	selector: &str,
) -> Result<Outcome> {
	let email = email.trim().to_lowercase();
	if email.len() > 320
		|| email.chars().any(|c| c.is_whitespace() || c.is_control())
		|| !email
			.split_once('@')
			.is_some_and(|(a, b)| !a.is_empty() && !b.is_empty() && !b.contains('@'))
	{
		return Err(Error::Invalid(
			"email must contain one @ with text on both sides".into(),
		));
	}
	let selector = aidash_domain::identity::tenancy::label(selector)?;
	let policy = config.dashboard_policy();
	let recovered = recover_tenant(pool, config, policy.as_ref(), &email, &selector).await;
	let identity = match &recovered {
		Ok(tenant) => tenant.identity,
		Err(_) => sole_eligible_holder(pool, &email).await?,
	};
	let deployment_administrator = BootstrapDeploymentAdministratorReservation::grant_or_hold(
		pool,
		policy.as_ref(),
		&email,
		identity,
	)
	.await?;
	match recovered {
		Ok(tenant) => Ok(Outcome {
			tenant: tenant.name,
			created: tenant.created,
			tenant_administrator: tenant.administrator,
			subject: tenant.subject,
			deployment_administrator,
		}),
		Err(error) => Err(not_recovered(error, &selector, deployment_administrator)),
	}
}

/// What the Tenant half of Bootstrap came to.
struct Recovered {
	name: String,
	created: bool,
	identity: Option<Uuid>,
	administrator: &'static str,
	subject: String,
}

/// Appends the Deployment Administrator outcome to a refusal of the Tenant.
/// Any other error is returned as it is.
fn not_recovered(error: Error, selector: &str, deployment_administrator: &str) -> Error {
	let note = |reason: String| {
		format!(
			"Tenant {selector:?} not recovered: {reason}. Deployment Administrator: {deployment_administrator}"
		)
	};
	match error {
		Error::Conflict(reason) => Error::Conflict(note(reason)),
		Error::NotFound(reason) => Error::NotFound(note(reason)),
		Error::Invalid(reason) => Error::Invalid(note(reason)),
		error => error,
	}
}

/// The identity the Deployment Administrator grant goes to when the Tenant
/// could not be recovered: the only enabled identity holding `email`, unless
/// another identity already spent one of the address's Host Access
/// reservations, as [`BootstrapDeploymentAdministratorReservation::consume`]
/// also refuses to split the two roles.
async fn sole_eligible_holder(pool: &Pool, email: &str) -> Result<Option<Uuid>> {
	pool.atomic(
		&TenancyScope::Deployment(DeploymentScopeReason::Bootstrap),
		async |tx| {
			let [(id, true)] = demo_reservations::holders(tx, email).await?[..] else {
				return Ok(None);
			};
			let split = DemoReservation::objects()
				.filter(DemoReservation::field_email().eq(email.to_owned()))
				.filter(DemoReservation::field_source().eq(HOST_ACCESS.to_owned()))
				.all_with_db(tx)
				.await?
				.iter()
				.any(|reservation| reservation.spent_by.is_some_and(|by| by != id));
			Ok((!split).then_some(id))
		},
	)
	.await
}

/// Names or creates the Tenant, then reserves `email` as its Tenant
/// Administrator and maps the identity holding it when there is one.
async fn recover_tenant(
	pool: &Pool,
	config: &Config,
	policy: Option<&AccountPolicy>,
	email: &str,
	selector: &str,
) -> Result<Recovered> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::Bootstrap);
	let (tenant, created) = pool
		.atomic(&scope, async |tx| {
			// Serialize all bootstrap selectors, including a name and its label.
			let (sql, values) = Query::select()
				.expr(SimpleExpr::FunctionCall(
					"pg_advisory_xact_lock".into_iden(),
					vec![Expr::value(892_895_i64).into()],
				))
				.build(PostgresQueryBuilder);
			TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
			let named = Tenant::objects()
				.filter(Tenant::field_name().eq(selector.to_owned()))
				.all_with_db(tx)
				.await?;
			if let Some(row) = named.into_iter().next() {
				return Ok((row.name, false));
			}
			let mut labels = Tenant::objects()
				.filter(Tenant::field_label().eq(Some(selector.to_owned())))
				.all_with_db(tx)
				.await?;
			if labels.len() > 1 {
				let mut names: Vec<_> = labels.into_iter().map(|t| t.name).collect();
				names.sort();
				return Err(Error::Conflict(format!(
					"multiple Tenants have label {selector:?}: {}",
					names.join(", ")
				)));
			}
			if let Some(row) = labels.pop() {
				return Ok((row.name, false));
			}
			let tenant = tenants::create(
				tx,
				config.deployment.as_str(),
				selector,
				false,
				Creator::HostAccess,
			)
			.await?;
			Ok((tenant.name, true))
		})
		.await?;
	let (identity, administrator, subject) = pool
		.atomic(&TenancyScope::tenant(&tenant)?, async |tx| {
			if crate::apps::identity::repositories::tenant_deletions::is_deleted(tx, &tenant)
				.await?
			{
				return Err(Error::Conflict("Tenant is deleted".into()));
			}
			let mut snapshot = AuthorizationBundle::lock_for_maintenance(tx, &tenant).await?;
			let earlier = DemoReservation::objects()
				.filter(DemoReservation::field_tenant().eq(tenant.clone()))
				.filter(DemoReservation::field_email().eq(email.to_owned()))
				.filter(DemoReservation::field_source().eq(HOST_ACCESS.to_owned()))
				.all_with_db(tx)
				.await?
				.pop();
			let used_names = AuthorizationRevision::subject_names(tx, &tenant).await?;
			let chosen = tenancy::bootstrap_subject(
				&snapshot.bundle,
				earlier.as_ref().map(|r| r.subject.as_str()),
				&used_names,
			);
			if let BootstrapSubject::New(name) = &chosen {
				tenant_administration::add_bootstrap_administrator(&mut snapshot.bundle, name)?;
				AuthorizationBundle::replace(
					tx,
					&tenant,
					snapshot.revision,
					serde_json::to_value(&snapshot.bundle)?,
					HOST_ACCESS,
					None,
				)
				.await?;
			}
			let subject = chosen.name().to_owned();
			// Moving the address to another subject starts its Reservation
			// over: the identity that spent it was mapped to the old subject.
			let moved = earlier.as_ref().is_some_and(|r| r.subject != subject);
			if let Some(earlier) = earlier.as_ref().filter(|_| moved) {
				let fields: Vec<FieldAssignment> = vec![
					(DemoReservation::field_subject(), subject.clone()).into(),
					(
						DemoReservation::field_spent_at(),
						None::<chrono::DateTime<chrono::Utc>>,
					)
						.into(),
					(DemoReservation::field_spent_by(), None::<uuid::Uuid>).into(),
				];
				DemoReservation::objects()
					.filter(DemoReservation::field_id().eq(earlier.id))
					.update_fields_with_conn(tx, fields)
					.await?;
			}
			let existing_reservation = earlier.is_some() && !moved;
			let reservation =
				DemoReservation::hold(tx, &tenant, email, &subject, HOST_ACCESS).await?;
			let holders = demo_reservations::holders(tx, email).await?;
			// As for Demo reservations, an ambiguous or disabled holder waits
			// for the first eligible identity to sign in again.
			let id = match &holders[..] {
				[(id, true)] => Some(*id),
				_ => reservation.spent_by.filter(|id| holders.contains(&(*id, true))),
			};
			if id.is_none() && reservation.spent_by.is_some() {
				return Err(Error::Conflict("the reservation was already spent and its identity cannot be identified uniquely by email".into()));
			}
			if id.is_some() && policy.is_none() {
				return Err(Error::Conflict(
					"dashboard sign-in must be configured to map an existing identity".into(),
				));
			}
			let has_mapping = if let Some(id) = id {
				DashboardMapping::objects()
					.filter(DashboardMapping::field_identity_id().eq(id))
					.filter(DashboardMapping::field_tenant().eq(tenant.clone()))
					.filter(DashboardMapping::field_subject().eq(subject.clone()))
					.filter(DashboardMapping::field_enabled().eq(true))
					.exists_with_db(tx)
					.await?
			} else {
				false
			};
			let already_mapped = id.is_some() && reservation.spent_by == id && has_mapping;
			if reservation.spent_at.is_some() && !already_mapped {
				let fields: Vec<FieldAssignment> = vec![
					(
						DemoReservation::field_spent_at(),
						None::<chrono::DateTime<chrono::Utc>>,
					)
						.into(),
					(DemoReservation::field_spent_by(), None::<uuid::Uuid>).into(),
				];
				DemoReservation::objects()
					.filter(DemoReservation::field_id().eq(reservation.id))
					.update_fields_with_conn(tx, fields)
					.await?;
			}
			if let Some(id) = id
				&& policy.is_some()
				&& !already_mapped
			{
				let result =
					DemoReservation::fulfil_in(tx, policy, &tenant, reservation.id, id).await?;
				if let crate::apps::identity::models::Fulfilment::Skipped(reason) = result {
					return Err(Error::Conflict(format!(
						"reservation cannot be fulfilled: {reason:?}"
					)));
				}
			}
			Ok((
				id,
				if has_mapping {
					"already mapped"
				} else if id.is_some() {
					"mapped"
				} else if existing_reservation && reservation.spent_at.is_none() {
					"already reserved"
				} else {
					"reserved"
				},
				subject,
			))
		})
		.await?;
	Ok(Recovered {
		name: tenant,
		created,
		identity,
		administrator,
		subject,
	})
}

pub struct Command;
#[async_trait::async_trait]
impl reinhardt::commands::CapabilityCommand for Command {
	fn cli(&self) -> clap::Command {
		clap::Command::new("bootstrap")
			.about("Bootstrap a Tenant and its administrators through Host Access")
			.arg(
				clap::Arg::new("email")
					.long("email")
					.required(true)
					.value_name("ADDRESS"),
			)
			.arg(
				clap::Arg::new("tenant")
					.long("tenant")
					.default_value("default")
					.value_name("NAME_OR_LABEL"),
			)
	}
	fn requirements(
		&self,
		_: &clap::ArgMatches,
	) -> Vec<reinhardt::commands::CapabilityRequirement> {
		Vec::new()
	}
	async fn execute(
		&self,
		matches: &clap::ArgMatches,
		_: &reinhardt::commands::CapabilityContext,
	) -> reinhardt::commands::CommandResult<()> {
		let result: Result<()> = async {
			let email = matches
				.get_one::<String>("email")
				.ok_or_else(|| Error::Invalid("bootstrap requires --email".into()))?;
			let tenant = matches
				.get_one::<String>("tenant")
				.ok_or_else(|| Error::Invalid("bootstrap requires --tenant".into()))?;
			let settings = crate::config::startup::load_settings()?;
			// Host Access bootstraps only a Self-hosted Deployment (ADR 0102
			// amendment); the Offering is known once the settings load.
			if crate::bootstrap::build::offering() != aidash_domain::offering::Offering::SelfHosted
			{
				return Err(Error::Invalid(
					"only a Self-hosted Deployment is bootstrapped from the host".into(),
				));
			}
			crate::bootstrap::require_migrated(&settings).await?;
			let config = Config::from_settings(&settings)?;
			let connection =
				crate::database::runtime::connect_command(&config.database_url, 4).await?;
			let pool = Pool::from(connection.into_postgres().ok_or(Error::Forbidden)?)
				.with_dashboard_policy(config.dashboard_policy());
			let outcome = bootstrap(&pool, &config, email, tenant).await;
			pool.close().await;
			let outcome = outcome?;
			println!(
				"Tenant {} {}. Tenant Administrator: {} as subject {}{}; Deployment Administrator: {}. Sign in with {} at {}.",
				outcome.tenant,
				if outcome.created {
					"created"
				} else {
					"already exists"
				},
				outcome.tenant_administrator,
				outcome.subject,
				if outcome.subject == aidash_domain::identity::tenancy::OWNER {
					""
				} else {
					" (owner is unusable)"
				},
				outcome.deployment_administrator,
				email,
				config.dashboard_origin
			);
			Ok(())
		}
		.await;
		result.map_err(|error| reinhardt::commands::CommandError::ExecutionError(error.to_string()))
	}
}
