//! How many bytes of files Aidash stores for each Tenant: capability objects,
//! channel attachments, Artifacts, Research snapshots and their originals, and
//! Research page images. Database rows beyond those files do not count. A build
//! that meters usage takes a sample every day and hands it to its usage port.
use crate::store::Store;
use crate::{Error, Result};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_application::usage::StorageSample;
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{QueryRow, execution::convert_values};
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder};
use serde::Deserialize;
use std::collections::BTreeMap;

/// Each stored file's size, per table: a SQL expression over one row.
const SOURCES: [(&str, &str); 6] = [
	("core_objects", "size"),
	("channel_attachments", "size_bytes"),
	("artifacts", "octet_length(content::text)"),
	("research_snapshots", "text_bytes"),
	("research_originals", "octet_length(bytes)"),
	("research_page_images", "octet_length(bytes)"),
];

#[derive(Deserialize)]
struct Total {
	tenant: String,
	bytes: i64,
}

/// One sample per registered Tenant, including zero stored bytes, at `now`.
/// The usage adapter owns the exemption for deleted Tenants.
pub async fn samples(store: &Store, now: DateTime<Utc>) -> Result<Vec<StorageSample>> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::UsageSampling);
	// A physical purge removes the last source row, but the active Tenant still
	// needs a zero sample to replace its previous nonzero usage.
	let tenants = store
		.pool
		.atomic(&scope, async |tx| {
			crate::apps::identity::models::Tenant::names(tx).await
		})
		.await?;
	let mut totals: BTreeMap<String, u64> = tenants
		.into_iter()
		.map(|tenant| (tenant.into_inner(), 0))
		.collect();
	for (table, size) in SOURCES {
		let (sql, values) = Query::select()
			.column(Alias::new("tenant"))
			.expr_as(
				Expr::cust(format!("COALESCE(SUM({size}), 0)::bigint")),
				Alias::new("bytes"),
			)
			.from(Alias::new(table))
			.group_by_col(Alias::new("tenant"))
			.build(PostgresQueryBuilder);
		let rows = store
			.pool
			.atomic(&scope, async |tx| {
				Ok(tx.fetch_all(&sql, convert_values(values)).await?)
			})
			.await?;
		for row in rows {
			let total: Total = serde_json::from_value(QueryRow::from_backend_row(row).data)?;
			let bytes = u64::try_from(total.bytes)
				.map_err(|_| Error::Invalid(format!("{table} holds a negative size")))?;
			*totals.entry(total.tenant).or_default() += bytes;
		}
	}
	Ok(totals
		.into_iter()
		.map(|(tenant, bytes)| StorageSample {
			tenant,
			sampled_at: now,
			bytes,
		})
		.collect())
}
