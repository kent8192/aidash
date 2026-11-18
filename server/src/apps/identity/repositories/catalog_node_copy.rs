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
use std::collections::{BTreeMap, BTreeSet};

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

/// Deployment-scoped rows of the Tenant's live Nodes. A deleted Node keeps its
/// rows until it is purged, but no Deployment-scoped change reaches it, so
/// they can be stale.
async fn live_deployment_rows(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
) -> Result<Vec<AuthorizationCatalog>> {
	let live: BTreeSet<String> = super::nodes::list_live(tx, tenant)
		.await?
		.into_iter()
		.map(|node| node.name.as_str().to_owned())
		.collect();
	Ok(AuthorizationCatalog::objects()
		.filter(AuthorizationCatalog::field_tenant().eq(tenant.to_owned()))
		.filter(AuthorizationCatalog::field_scope().eq("deployment".to_owned()))
		.order_by(&["entry_id", "entry_version", "node"])
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?
		.into_iter()
		.filter(|row| live.contains(&row.node))
		.collect())
}

/// What a Deployment-scoped row grants, with the Node's own Agent subjects
/// named relative to it so rows of different Nodes compare.
fn consent(
	row: &AuthorizationCatalog,
	deployment: &DeploymentId,
) -> Result<(bool, bool, Option<CatalogRestriction>)> {
	let prefix = format!("{}/agents/", deployment.node(NodeName::parse(&row.node)?));
	let mut restriction: Option<CatalogRestriction> = row
		.restriction
		.clone()
		.map(|value| serde_json::from_value(value.0))
		.transpose()?;
	if let Some(restriction) = &mut restriction {
		restriction.subjects = restriction
			.subjects
			.iter()
			.map(|subject| {
				subject
					.strip_prefix(&prefix)
					.map_or_else(|| subject.clone(), |rest| format!("./agents/{rest}"))
			})
			.collect();
	}
	Ok((row.enabled, row.carry_over, restriction))
}

/// The caller holds the exclusive policy and Node inventory locks.
pub(crate) async fn copy(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	deployment: &DeploymentId,
	target: &NodeName,
	snapshot: &mut Snapshot,
) -> Result<()> {
	// Withdrawn rows take part, so a version withdrawn on one live Node is
	// never copied from another that still holds it.
	let rows = live_deployment_rows(tx, tenant).await?;
	// `None` marks a version whose Nodes disagree. Creating a Node never fails
	// on that: the new Node gets no approval for it and asks again.
	let mut copies = BTreeMap::<(String, String), Option<CopyRow>>::new();
	for row in rows {
		let source = NodeName::parse(&row.node)?;
		if &source == target {
			continue;
		}
		if !row.enabled {
			// Withdrawn on a live Node: never copied from another.
			copies.insert((row.entry_key, row.entry_version), None);
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
			let Some(mut subject) = snapshot.bundle.subjects.get(&original).cloned() else {
				tracing::warn!(
					tenant,
					entry = %row.entry_key,
					version = %row.entry_version,
					"deployment approval has no source agent subject; not copied to the new Node"
				);
				copies.insert((row.entry_key, row.entry_version), None);
				continue;
			};
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
			let Some(existing) = existing else {
				continue;
			};
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
				tracing::warn!(
					tenant,
					entry = %key.0,
					version = %key.1,
					"deployment approvals differ across Nodes; not copied to the new Node"
				);
				copies.insert(key, None);
			}
		} else {
			copies.insert(key, Some(candidate));
		}
	}
	let mut policy_changed = false;
	for row in copies.values().flatten() {
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

/// Whether `row` grants what every other live Node's row of its version
/// grants. A version no other live Node holds Deployment-wide agrees.
fn agrees_with_peers(
	row: &AuthorizationCatalog,
	others: &[AuthorizationCatalog],
	deployment: &DeploymentId,
) -> Result<bool> {
	let granted = consent(row, deployment)?;
	for peer in others
		.iter()
		.filter(|peer| peer.entry_key == row.entry_key && peer.entry_version == row.entry_version)
	{
		if consent(peer, deployment)? != granted {
			return Ok(false);
		}
	}
	Ok(true)
}

/// Narrow an update to `row`'s approval at `revision`.
fn at_revision(
	query: &mut reinhardt::query::UpdateStatement,
	row: &AuthorizationCatalog,
	revision: i64,
) {
	query
		.and_where(Expr::col(Alias::new("tenant")).eq(row.tenant.clone()))
		.and_where(Expr::col(Alias::new("node")).eq(row.node.clone()))
		.and_where(Expr::col(Alias::new("entry_id")).eq(row.entry_key.clone()))
		.and_where(Expr::col(Alias::new("entry_version")).eq(row.entry_version.clone()))
		.and_where(Expr::col(Alias::new("revision")).eq(revision));
}

/// A restored Node missed every Deployment-scoped change made while it was
/// deleted. Each of its Deployment-scoped rows that no longer grants what the
/// other live Nodes' rows of that version grant leaves the Deployment scope,
/// withdrawn if it was enabled, so restoring a Node never brings back an
/// approval withdrawn, narrowed or stopped from carrying on every Node (#873).
/// A version no other live Node holds Deployment-wide is left as it is.
/// The caller holds the exclusive policy and Node inventory locks.
pub(crate) async fn reconcile_restored(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	deployment: &DeploymentId,
	restored: &NodeName,
) -> Result<()> {
	let (own, others): (Vec<_>, Vec<_>) = live_deployment_rows(tx, tenant)
		.await?
		.into_iter()
		.partition(|row| row.node == restored.as_str());
	for row in own {
		if agrees_with_peers(&row, &others, deployment)? {
			continue;
		}
		tracing::warn!(
			tenant,
			node = %restored,
			entry = %row.entry_key,
			version = %row.entry_version,
			"restored Node's Deployment approval differs from the live Nodes; kept to this Node"
		);
		let revision = if row.enabled {
			row.revision + 1
		} else {
			row.revision
		};
		let mut update = Query::update();
		update
			.table(Alias::new("authorization_catalog"))
			.value(Alias::new("enabled"), false)
			.value(Alias::new("revision"), revision)
			.value(Alias::new("scope"), "node");
		at_revision(&mut update, &row, row.revision);
		let (sql, params) = update.build(PostgresQueryBuilder);
		if tx
			.execute(&sql, convert_values(params))
			.await?
			.rows_affected
			!= 1
		{
			return Err(Error::Conflict("catalog revision changed".into()));
		}
		if row.enabled {
			// The withdrawal is a revision of its own, like any other.
			let mut current = Query::select();
			current
				.columns(
					[
						"tenant",
						"node",
						"entry_id",
						"entry_version",
						"revision",
						"enabled",
						"restriction",
					]
					.map(Alias::new),
				)
				.expr(Expr::value("node-restore"))
				.columns(
					[
						"carry_over",
						"scope",
						"carried_from_version",
						"carried_from_approver",
					]
					.map(Alias::new),
				)
				.from(Alias::new("authorization_catalog"))
				.and_where(Expr::col(Alias::new("tenant")).eq(row.tenant.clone()))
				.and_where(Expr::col(Alias::new("node")).eq(row.node.clone()))
				.and_where(Expr::col(Alias::new("entry_id")).eq(row.entry_key.clone()))
				.and_where(Expr::col(Alias::new("entry_version")).eq(row.entry_version.clone()))
				.and_where(Expr::col(Alias::new("revision")).eq(revision));
			let (sql, params) = Query::insert()
				.into_table(Alias::new("authorization_catalog_history"))
				.columns(
					[
						"tenant",
						"node",
						"entry_id",
						"entry_version",
						"revision",
						"enabled",
						"restriction",
						"actor",
						"carry_over",
						"scope",
						"carried_from_version",
						"carried_from_approver",
					]
					.map(Alias::new),
				)
				.from_subquery(current.to_owned())
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(params)).await?;
		} else {
			let mut update = Query::update();
			update
				.table(Alias::new("authorization_catalog_history"))
				.value(Alias::new("scope"), "node");
			at_revision(&mut update, &row, row.revision);
			let (sql, params) = update.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(params)).await?;
		}
	}
	Ok(())
}
