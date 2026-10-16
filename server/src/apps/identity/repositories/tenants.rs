//! Creating Tenants. Every Tenant, including a Personal Tenant and one a
//! Deployment Administrator creates, gets a root record under a name Aidash
//! draws (ADR 0050) and the same initial policy.
use crate::apps::identity::models::{
	AdministrationActor, AdministrationEvent, AuthorizationBundle, AuthorizationCredential,
	DashboardMapping, Tenant, database_time,
};
use crate::{Error, Result};
use aidash_domain::identity::tenancy::{self, OWNER, TenantName};
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{AtomicTransaction, Model, OrmExecutor};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, IntoValue, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};
use uuid::Uuid;

/// Who creates a Tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Creator {
	/// A Tenant Creator: the External Identity receives the first Mapping.
	Identity(Uuid),
	/// A Deployment Administrator, through the bearer token (no identity) or a
	/// browser session. Nobody is mapped into the new Tenant.
	DeploymentAdministrator(Option<Uuid>),
}

impl Creator {
	fn kind(self) -> &'static str {
		match self {
			Self::Identity(_) => "identity",
			Self::DeploymentAdministrator(_) => "deployment_administrator",
		}
	}
	fn identity(self) -> Option<Uuid> {
		match self {
			Self::Identity(identity) | Self::DeploymentAdministrator(Some(identity)) => {
				Some(identity)
			}
			Self::DeploymentAdministrator(None) => None,
		}
	}
	/// The first policy revision's actor.
	fn revision_actor(self) -> String {
		match self {
			Self::Identity(identity) => format!("oidc:{identity}"),
			Self::DeploymentAdministrator(_) => "deployment_administrator".into(),
		}
	}
}

/// A created Tenant and, for a Tenant Creator, its first Mapping.
#[derive(Debug, Clone)]
pub(crate) struct Created {
	pub name: String,
	pub label: String,
	pub created_at: DateTime<Utc>,
	pub mapping: Option<Uuid>,
}

/// Serializes one External Identity's creations so concurrent requests at a
/// limit cannot all pass the count.
pub(crate) async fn lock_creator(tx: &mut dyn TransactionExecutor, identity: Uuid) -> Result<()> {
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![SimpleExpr::FunctionCall(
				"hashtextextended".into_iden(),
				vec![
					Expr::value(identity.to_string()).into(),
					Expr::value(23_800_238_i64).into(),
				],
			)],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

/// The creation times within the last hour, and the number not purged, of the
/// Tenants `identity` created through the Tenant creation API. Personal
/// Tenants never count. Both are bounded queries: the total is counted in
/// the database and only the last hour's rows are read, so the work held
/// under the creator's lock does not grow with every Tenant it ever made.
pub(crate) async fn creations(
	tx: &mut AtomicTransaction,
	identity: Uuid,
	now: DateTime<Utc>,
) -> Result<(Vec<DateTime<Utc>>, u32)> {
	let created = || {
		Tenant::objects()
			.filter(Tenant::field_created_by_kind().eq("identity".to_owned()))
			.filter(Tenant::field_created_by_identity().eq(Some(identity)))
			.filter(Tenant::field_personal().eq(false))
	};
	let total = created().count_with_db(tx).await?;
	let recent = created()
		.filter(Tenant::field_created_at().gt(now - chrono::Duration::hours(1)))
		.all_with_db(tx)
		.await?
		.into_iter()
		.map(|tenant| tenant.created_at)
		.collect();
	Ok((recent, u32::try_from(total).unwrap_or(u32::MAX)))
}

/// Inserts a root record under a freshly drawn name. A collision draws again;
/// a name taken once is never reused, because root records are never deleted.
async fn insert_root(
	tx: &mut dyn TransactionExecutor,
	label: &str,
	personal: bool,
	creator: Creator,
) -> Result<(String, DateTime<Utc>)> {
	let now = database_time(tx).await?;
	for _ in 0..8 {
		let name = TenantName::from_entropy(*Uuid::new_v4().as_bytes()).into_string();
		let (sql, values) = Query::insert()
			.into_table(Alias::new(Tenant::table_name()))
			.columns(
				[
					"name",
					"label",
					"personal",
					"created_at",
					"created_by_kind",
					"created_by_identity",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value(name.clone()),
				IntoValue::into_value(label.to_owned()),
				IntoValue::into_value(personal),
				IntoValue::into_value(now),
				IntoValue::into_value(creator.kind()),
				IntoValue::into_value(creator.identity()),
			])
			.on_conflict(
				OnConflict::column(Alias::new("name"))
					.do_nothing()
					.to_owned(),
			)
			.build(PostgresQueryBuilder);
		if tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected
			== 1
		{
			return Ok((name, now));
		}
	}
	Err(Error::Conflict("no free Tenant name was drawn".into()))
}

/// Creates a Tenant with the initial policy. A Tenant Creator is mapped to the
/// subject `owner` in `administrators` through a non-exported browser
/// credential, as an approved Registration Request would be.
pub(crate) async fn create(
	tx: &mut AtomicTransaction,
	label: &str,
	personal: bool,
	creator: Creator,
) -> Result<Created> {
	let label = tenancy::label(label)?;
	let (name, created_at) = insert_root(tx, &label, personal, creator).await?;
	let bundle = tenancy::initial_policy(&name, matches!(creator, Creator::Identity(_)))?;
	AuthorizationBundle::replace(
		tx,
		&name,
		0,
		serde_json::to_value(&bundle)?,
		&creator.revision_actor(),
	)
	.await?;
	let mapping = match creator {
		Creator::Identity(identity) => Some(map_owner(tx, &name, identity).await?),
		Creator::DeploymentAdministrator(_) => None,
	};
	let actor = match creator {
		Creator::Identity(identity) => AdministrationActor::Identity {
			identity,
			mapping: None,
		},
		Creator::DeploymentAdministrator(None) => {
			AdministrationActor::DeploymentAdministratorBearer
		}
		Creator::DeploymentAdministrator(Some(identity)) => {
			AdministrationActor::DeploymentAdministrator { identity }
		}
	};
	actor
		.record(
			tx,
			AdministrationEvent {
				action: "tenant.create",
				// Only a Tenant Creator is the identity the creation concerns;
				// a Deployment Administrator's identity is the actor alone.
				identity: match creator {
					Creator::Identity(identity) => Some(identity),
					Creator::DeploymentAdministrator(_) => None,
				},
				registration: None,
				mapping,
				tenant: Some(&name),
				subject: mapping.map(|_| OWNER),
				invitation: None,
			},
		)
		.await?;
	Ok(Created {
		name,
		label,
		created_at,
		mapping,
	})
}

async fn map_owner(tx: &mut AtomicTransaction, tenant: &str, identity: Uuid) -> Result<Uuid> {
	// This non-exported credential uses the existing dashboard marker so
	// credential listing and revocation continue to exclude browser secrets.
	let credential = AuthorizationCredential::issue(
		tx,
		tenant,
		OWNER,
		crate::apps::identity::services::oidc::digest(
			&crate::apps::identity::services::oidc::random_secret(),
		),
		3650 * 24 * 60 * 60,
		"dashboard-oidc",
	)
	.await?;
	let mapping = DashboardMapping::build()
		.id(Uuid::new_v4())
		.identity_id(identity)
		.tenant(tenant.to_owned())
		.subject(OWNER.to_owned())
		.credential_id(credential.id)
		.enabled(true)
		.revision(1)
		.finish();
	DashboardMapping::objects()
		.create_with_conn(tx, &mapping)
		.await?;
	Ok(mapping.id)
}

/// Root records listed per page.
const PAGE: usize = 200;

/// One page of root records, newest first, for Deployment Administrators.
pub(crate) async fn list(tx: &mut AtomicTransaction, offset: u64) -> Result<Vec<Tenant>> {
	Ok(Tenant::objects()
		.order_by(&["-created_at", "name"])
		.limit(PAGE)
		.offset(usize::try_from(offset).unwrap_or(usize::MAX))
		.all_with_db(tx)
		.await?)
}

/// The labels of `names`, for showing Tenants to people mapped into them.
pub(crate) async fn labels<E: OrmExecutor>(
	tx: &mut E,
	names: Vec<String>,
) -> Result<std::collections::BTreeMap<String, Option<String>>> {
	if names.is_empty() {
		return Ok(Default::default());
	}
	Ok(Tenant::objects()
		.filter(Tenant::field_name().is_in(names))
		.all_with_db(tx)
		.await?
		.into_iter()
		.map(|tenant| (tenant.name, tenant.label))
		.collect())
}

/// Changes a Tenant label. The caller has checked `tenant.update`.
pub(crate) async fn relabel(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	label: &str,
) -> Result<String> {
	let label = tenancy::label(label)?;
	let (sql, values) = Query::update()
		.table(Alias::new(Tenant::table_name()))
		.value_expr(Alias::new("label"), Expr::value(label.clone()))
		.and_where(Expr::col("name").eq(Expr::value(tenant)))
		.build(PostgresQueryBuilder);
	if tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		!= 1
	{
		return Err(Error::NotFound("tenant".into()));
	}
	Ok(label)
}
