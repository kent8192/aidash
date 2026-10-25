//! Importing an Agent Plugin: fetch it, keep the Tenant's copy and return the reference
//! an Agent definition records (ADR 0072).
use crate::{
	Error, Result,
	apps::registry::{
		repositories::agent_plugins,
		serializers::agent_plugin_import::{
			AgentPluginImportResult, ImportedAgentPlugin, PluginImportRequest,
		},
	},
	authorization::{access::Access, identity::Actor},
	federation::Federation,
};
use aidash_application::{
	ports::accounts::Authority,
	registry::agent_plugin_import::{AgentPluginImport, PluginImport},
};
use aidash_domain::agent_plugin::{AgentPluginRef, LoadedPlugin, PluginFile};

/// Needs `registry.create` on an Agent, because a copy only serves Agent definitions. The
/// check comes before any network access, and the copy is stored in a second transaction that
/// decides again, so the fetch holds no database connection.
pub async fn import(
	runtime: &Federation,
	source: &AgentPluginImport,
	actor: Actor,
	request: PluginImportRequest,
) -> Result<AgentPluginImportResult> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&runtime.store, &identity).await?;
	let result = async {
		require_agent_create(&mut access).await?;
		require_library(runtime, &mut access, &request.url).await
	}
	.await;
	access.finish(result).await?;
	match source.import(request).await? {
		PluginImport::Choose(roots) => Ok(AgentPluginImportResult {
			plugins: roots,
			selected: None,
		}),
		PluginImport::Loaded {
			entitlement_expires_at,
			source,
			root,
			denied,
			plugin,
			files,
		} => {
			let mut access = Access::begin(&runtime.store, &identity).await?;
			let result = async {
				require_library(runtime, &mut access, &source).await?;
				let selected = store(
					&mut access,
					&runtime.registry.validation(),
					&source,
					&plugin,
					&files,
					denied,
				)
				.await?;
				if let Some(marketplace) =
					aidash_domain::agent_plugin::MarketplaceSource::parse(&source)
				{
					crate::apps::registry::repositories::marketplace_recalls::receipt(
						&mut **access.tx,
						&marketplace,
						&plugin.digest,
						entitlement_expires_at,
					)
					.await?;
				}
				Ok(selected)
			}
			.await;
			let selected = access.finish(result).await?;
			Ok(AgentPluginImportResult {
				plugins: vec![root],
				selected: Some(selected),
			})
		}
	}
}

async fn require_agent_create(access: &mut Access) -> Result<()> {
	let resource = access.resource(
		"registry",
		format!(
			"agent/{}",
			aidash_domain::identity::tenant_administration::COLLECTION
		),
		serde_json::json!({"kind": "agent"}),
	);
	access.require(&resource, "registry.create").await
}

/// Keeps the copy in the caller's Tenant and builds the reference from the stored row, so that
/// an import after a later change to how plugins load still matches definitions registered
/// earlier.
async fn store(
	access: &mut Access,
	validation: &aidash_application::registry::DefinitionValidation,
	source: &str,
	plugin: &LoadedPlugin,
	files: &[PluginFile],
	denied: Vec<String>,
) -> Result<ImportedAgentPlugin> {
	require_agent_create(access).await?;
	validation.validate_agent_plugin(&AgentPluginRef::of(plugin, source))?;
	let stored = agent_plugins::insert(&mut **access.tx, source, plugin, files, &denied).await?;
	let reference = AgentPluginRef::of(&stored.plugin, &stored.source);
	reference.validate()?;
	Ok(ImportedAgentPlugin { reference, denied })
}

/// A personal Library may enter its owner's Personal Tenant by default. Other
/// Tenants need their own explicit policy; configuration alone grants no right.
async fn require_library(runtime: &Federation, access: &mut Access, source: &str) -> Result<()> {
	if !source.starts_with("marketplace://") {
		return Ok(());
	}
	let connection = runtime
		.config
		.marketplace
		.connection(&access.identity.tenant)
		.ok_or(Error::Forbidden)?;
	if connection.owner_tenant_id.as_deref() == Some(&access.identity.tenant) {
		return Ok(());
	}
	use reinhardt::db::orm::execution::convert_values;
	use reinhardt::query::{
		Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	if let Some(identity) = connection.personal_identity_id {
		let (sql, values) = Query::select()
			.column(Alias::new("identity_id"))
			.from(Alias::new("personal_tenants"))
			.and_where(Expr::col("identity_id").eq(Expr::value(identity)))
			.and_where(Expr::col("tenant").eq(Expr::value(&access.identity.tenant)))
			.limit(1)
			.build(PostgresQueryBuilder);
		if access
			.tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.is_some()
			&& let Some(authority) = runtime.config.common_auth.as_ref()
			&& authority
				.origin_binding(identity)
				.await
				.is_ok_and(|binding| {
					binding.identity_id == identity
						&& binding.account_id == Some(connection.owner_account_id)
				}) {
			return Ok(());
		}
	}
	let resource = access.resource(
		"marketplace_library",
		format!("account/{}", connection.owner_account_id),
		serde_json::json!({"owner_account_id":connection.owner_account_id}),
	);
	let action = "marketplace.personal_library.import";
	access.require(&resource, action).await?;
	// The built-in Tenant administrator wildcard is not an explicit opt-in to
	// bringing a person's paid Library into a different Tenant.
	let mut explicit = access.snapshot.bundle.clone();
	explicit.policies.retain(|policy| {
		policy.effect == aidash_domain::policy::Effect::Deny || policy.actions.contains(action)
	});
	let decisions: Vec<_> = access
		.subjects
		.iter()
		.map(|subject| {
			let input = access.evaluation(subject, &resource, action);
			let mut decision = explicit.evaluate(&input);
			decision.revision = access.snapshot.revision;
			(input, decision)
		})
		.collect();
	let allowed = !decisions.is_empty() && decisions.iter().all(|(_, decision)| decision.allowed);
	access.record(&decisions).await?;
	if allowed {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}
