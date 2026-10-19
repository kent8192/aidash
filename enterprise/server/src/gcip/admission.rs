//! How each GCIP External Identity was admitted: through the Shared Pool or
//! bound to a named Tenant (#240, #266). Deployment-wide: it names a Tenant
//! but belongs to none.
use aidash_application::tenancy::TenancyScope;
use aidash_server::Result;
use aidash_server::federation::Federation;
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::model;
use reinhardt::query::{
	Alias, IntoValue, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "gcip", table_name = "gcip_admissions")]
#[derive(Serialize, Deserialize)]
pub struct GcipAdmission {
	/// The External Identity, whose GCIP Tenant is part of its issuer.
	#[field(primary_key = true)]
	pub identity_id: Uuid,
	#[field(field_type = "text")]
	pub gcip_tenant: String,
	/// The Tenant a Tenant Binding restricted it to; none for the Shared Pool.
	#[field(field_type = "text", null = true)]
	pub tenant: Option<String>,
	pub admitted_at: DateTime<Utc>,
}

/// Record the first admission only: an identity remembers the pool status it
/// was created under, so a later change can disable it.
pub async fn record(
	f: &Federation,
	identity: Uuid,
	gcip_tenant: &str,
	tenant: Option<&str>,
) -> Result<()> {
	let (sql, values) = Query::insert()
		.into_table(Alias::new(GcipAdmission::table_name()))
		.columns(["identity_id", "gcip_tenant", "tenant", "admitted_at"].map(Alias::new))
		.values_panic([
			IntoValue::into_value(identity),
			IntoValue::into_value(gcip_tenant.to_owned()),
			IntoValue::into_value(tenant.map(str::to_owned)),
			IntoValue::into_value(Utc::now()),
		])
		.on_conflict(
			OnConflict::column(Alias::new("identity_id"))
				.do_nothing()
				.to_owned(),
		)
		.build(PostgresQueryBuilder);
	f.store
		.pool
		.atomic(&TenancyScope::NoTenant, async |tx| {
			TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
			Ok(())
		})
		.await
}
