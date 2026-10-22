//! Step 1 of a Demo data load: one Tenant for each structure key.
use super::Structure;
use crate::Result;
use crate::apps::identity::models::DemoTenant;
use crate::apps::identity::repositories::tenants::{self, Creator};
use crate::database::native::Pool;
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use reinhardt::db::orm::Model;
use std::collections::BTreeMap;

/// Finds or creates the Tenant of every structure key and returns each key's
/// Tenant name. A Tenant and the `demo_tenants` row that names it commit
/// together, so a rerun never creates a second Tenant for a key. The Tenant's
/// name is drawn inside the transaction (ADR 0050), so the scope is the one
/// Tenant creation uses.
pub(super) async fn ensure(
	pool: &Pool,
	node: &str,
	structure: &Structure,
) -> Result<BTreeMap<String, String>> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::TenantCreation);
	let mut names = BTreeMap::new();
	for tenant in &structure.tenants {
		let name = pool
			.atomic(&scope, async |tx| {
				if let Some(existing) = DemoTenant::objects()
					.filter(DemoTenant::field_key().eq(tenant.key.clone()))
					.all_with_db(tx)
					.await?
					.pop()
				{
					return Ok(existing.tenant);
				}
				let created = tenants::create(
					tx,
					node,
					&tenant.label,
					false,
					Creator::DeploymentAdministrator(None),
				)
				.await?;
				let named = DemoTenant::build()
					.key(tenant.key.clone())
					.tenant(created.name.clone())
					.finish();
				DemoTenant::objects().create_with_conn(tx, &named).await?;
				Ok(created.name)
			})
			.await?;
		names.insert(tenant.key.clone(), name);
	}
	Ok(names)
}
