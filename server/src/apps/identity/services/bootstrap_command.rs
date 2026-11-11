//! Host Access bootstrap of a Tenant and its administrators.
use crate::apps::identity::models::{
	AuthorizationBundle, BootstrapDeploymentAdministratorReservation, DashboardMapping,
	DemoReservation, Tenant,
};
use crate::apps::identity::repositories::tenants::{self, Creator};
use crate::config::Config;
use crate::database::native::Pool;
use crate::{Error, Result};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::identity::tenancy::{ADMINISTRATORS, OWNER};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, query::FieldAssignment};
use reinhardt::query::{
	Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use std::collections::BTreeSet;

#[derive(Debug)]
pub struct Outcome {
	pub tenant: String,
	pub created: bool,
	pub tenant_administrator: &'static str,
	pub deployment_administrator: &'static str,
}

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
	let selector = selector.as_str();
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
	let policy = config.dashboard_policy();
	let (identity, tenant_administrator) = pool
		.atomic(&TenancyScope::tenant(&tenant)?, async |tx| {
			if crate::apps::identity::repositories::tenant_deletions::is_deleted(tx, &tenant)
				.await?
			{
				return Err(Error::Conflict("Tenant is deleted".into()));
			}
			let mut snapshot = AuthorizationBundle::lock_for_maintenance(tx, &tenant).await?;
			if !snapshot.bundle.subjects.contains_key(OWNER) {
				aidash_domain::identity::tenant_administration::add_user_subject(
					&mut snapshot.bundle,
					OWNER,
					&BTreeSet::from([ADMINISTRATORS.to_owned()]),
				)?;
				AuthorizationBundle::replace(
					tx,
					&tenant,
					snapshot.revision,
					serde_json::to_value(&snapshot.bundle)?,
					"host_access",
					None,
				)
				.await?;
			} else {
				let owner = &snapshot.bundle.subjects[OWNER];
				if owner.kind != aidash_domain::policy::SubjectKind::User
					|| !owner.enabled
					|| !owner.groups.contains(ADMINISTRATORS)
				{
					return Err(Error::Conflict(
						"owner subject is not an enabled administrator".into(),
					));
				}
			}
			let existing_reservation = DemoReservation::objects()
				.filter(DemoReservation::field_tenant().eq(tenant.clone()))
				.filter(DemoReservation::field_email().eq(email.clone()))
				.filter(DemoReservation::field_source().eq("host_access".to_owned()))
				.exists_with_db(tx)
				.await?;
			let reservation =
				DemoReservation::hold(tx, &tenant, &email, OWNER, "host_access").await?;
			if reservation.source != "host_access" || reservation.subject != OWNER {
				return Err(Error::Conflict(
					"email already has another Tenant reservation".into(),
				));
			}
			let holders =
				crate::apps::identity::models::demo_reservations::holders(tx, &email).await?;
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
					.filter(DashboardMapping::field_subject().eq(OWNER.to_owned()))
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
					DemoReservation::fulfil_in(tx, policy.as_ref(), &tenant, reservation.id, id)
						.await?;
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
			))
		})
		.await?;
	let deployment_administrator =
		BootstrapDeploymentAdministratorReservation::grant_or_hold(pool, &email, identity).await?;
	Ok(Outcome {
		tenant,
		created,
		tenant_administrator,
		deployment_administrator,
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
				"Tenant {} {}. Tenant Administrator: {}; Deployment Administrator: {}. Sign in with {} at {}.",
				outcome.tenant,
				if outcome.created {
					"created"
				} else {
					"already exists"
				},
				outcome.tenant_administrator,
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
