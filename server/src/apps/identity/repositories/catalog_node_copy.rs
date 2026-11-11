//! Copy Deployment-scoped Catalog Approvals while a Node is created.
use crate::apps::identity::models::{
	AuthorizationBundle, AuthorizationCatalog, AuthorizationCatalogHistory,
};
use crate::apps::registry::models::Definition;
use crate::{Error, Result};
use aidash_application::authorization::Snapshot;
use aidash_domain::{
	identity::catalog::CatalogRestriction,
	nodes::{DeploymentId, NodeName},
	policy::Subject,
};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{Model, execution::convert_values},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use std::collections::BTreeMap;

struct CopyRow {
	id: String,
	version: String,
	enabled: bool,
	carry_over: bool,
	from_version: Option<String>,
	from_approver: Option<String>,
	actor: String,
	provider_warning_acknowledgment: Option<String>,
	restriction: Option<CatalogRestriction>,
	agent: Option<Subject>,
}

/// The caller holds the exclusive policy and Node inventory locks.
pub(crate) async fn copy(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	deployment: &DeploymentId,
	target: &NodeName,
	snapshot: &mut Snapshot,
) -> Result<()> {
	let rows = AuthorizationCatalog::objects()
		.filter(AuthorizationCatalog::field_tenant().eq(tenant.to_owned()))
		.filter(AuthorizationCatalog::field_scope().eq("deployment".to_owned()))
		.order_by(&["entry_id", "entry_version", "node"])
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?;
	let mut copies = BTreeMap::<(String, String), CopyRow>::new();
	for row in rows {
		let source = NodeName::parse(&row.node)?;
		if &source == target {
			continue;
		}
		let history = AuthorizationCatalogHistory::objects()
			.filter(AuthorizationCatalogHistory::field_tenant().eq(tenant.to_owned()))
			.filter(AuthorizationCatalogHistory::field_node().eq(row.node.clone()))
			.filter(AuthorizationCatalogHistory::field_entry_key().eq(row.entry_key.clone()))
			.filter(
				AuthorizationCatalogHistory::field_entry_version().eq(row.entry_version.clone()),
			)
			.filter(AuthorizationCatalogHistory::field_revision().eq(row.revision))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.ok_or_else(|| Error::Conflict("deployment approval has no current history".into()))?;
		let old_node = deployment.node(source.clone());
		let new_node = deployment.node(target.clone());
		let old_prefix = format!("{old_node}/agents/");
		let new_prefix = format!("{new_node}/agents/");
		let mut restriction: Option<CatalogRestriction> = row
			.restriction
			.map(|value| serde_json::from_value(value.0))
			.transpose()?;
		if let Some(restriction) = &mut restriction {
			restriction.subjects = restriction
				.subjects
				.iter()
				.map(|subject| {
					subject
						.strip_prefix(&old_prefix)
						.map_or_else(|| subject.clone(), |rest| format!("{new_prefix}{rest}"))
				})
				.collect();
		}
		let definition = Definition::read_in(tx, &row.entry_key, &row.entry_version).await?;
		let agent = if definition.kind == "agent" {
			let original =
				aidash_domain::qualified_agent(&old_node, &row.entry_key, &row.entry_version);
			let mut subject = snapshot
				.bundle
				.subjects
				.get(&original)
				.cloned()
				.ok_or_else(|| {
					Error::Conflict("deployment approval has no source agent subject".into())
				})?;
			subject.node = Some(target.clone());
			Some(subject)
		} else {
			None
		};
		let candidate = CopyRow {
			id: row.entry_key,
			version: row.entry_version,
			enabled: row.enabled,
			carry_over: row.carry_over,
			from_version: row.carried_from_version,
			from_approver: row.carried_from_approver,
			actor: history.actor,
			provider_warning_acknowledgment: history.provider_warning_acknowledgment,
			restriction,
			agent,
		};
		let key = (candidate.id.clone(), candidate.version.clone());
		if let Some(existing) = copies.get(&key) {
			if existing.enabled != candidate.enabled
				|| existing.carry_over != candidate.carry_over
				|| existing.from_version != candidate.from_version
				|| existing.from_approver != candidate.from_approver
				|| existing.actor != candidate.actor
				|| existing.provider_warning_acknowledgment
					!= candidate.provider_warning_acknowledgment
				|| existing.restriction != candidate.restriction
				|| serde_json::to_value(&existing.agent)? != serde_json::to_value(&candidate.agent)?
			{
				return Err(Error::Conflict(
					"deployment approvals differ across Nodes".into(),
				));
			}
		} else {
			copies.insert(key, candidate);
		}
	}
	let mut policy_changed = false;
	for row in copies.values() {
		if let Some(subject) = &row.agent {
			let name = aidash_domain::qualified_agent(
				&deployment.node(target.clone()),
				&row.id,
				&row.version,
			);
			if snapshot
				.bundle
				.subjects
				.insert(name, subject.clone())
				.is_some()
			{
				return Err(Error::Conflict(
					"new Node agent subject already exists".into(),
				));
			}
			policy_changed = true;
		}
		let restriction = match &row.restriction {
			Some(value) => Expr::value(serde_json::to_value(value)?).cast_as("jsonb"),
			None => Expr::value(Option::<serde_json::Value>::None).into(),
		};
		for history in [false, true] {
			let table = if history {
				"authorization_catalog_history"
			} else {
				"authorization_catalog"
			};
			let mut columns = vec![
				"tenant",
				"node",
				"entry_id",
				"entry_version",
				"enabled",
				"revision",
				"restriction",
				"carry_over",
				"scope",
				"carried_from_version",
				"carried_from_approver",
			];
			let mut values = Query::select();
			values
				.expr(Expr::value(tenant.to_owned()))
				.expr(Expr::value(target.as_str()))
				.expr(Expr::value(row.id.clone()))
				.expr(Expr::value(row.version.clone()))
				.expr(Expr::value(row.enabled))
				.expr(Expr::value(1_i64))
				.expr(restriction.clone())
				.expr(Expr::value(row.carry_over))
				.expr(Expr::value("deployment"))
				.expr(Expr::value(row.from_version.clone()))
				.expr(Expr::value(row.from_approver.clone()));
			if history {
				columns.push("actor");
				columns.push("provider_warning_acknowledgment");
				values.expr(Expr::value(row.actor.clone()));
				values.expr(Expr::value(row.provider_warning_acknowledgment.clone()));
			}
			let (sql, params) = Query::insert()
				.into_table(Alias::new(table))
				.columns(columns.into_iter().map(Alias::new))
				.from_subquery(values.to_owned())
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(params)).await?;
		}
	}
	if policy_changed {
		snapshot.bundle.validate()?;
		snapshot.revision = AuthorizationBundle::replace(
			tx,
			tenant,
			snapshot.revision,
			serde_json::to_value(&snapshot.bundle)?,
			"deployment-scope-copy",
			None,
		)
		.await?;
	}
	Ok(())
}
