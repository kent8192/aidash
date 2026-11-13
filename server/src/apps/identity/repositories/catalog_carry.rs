//! Registration-time approval carry-over, within the caller's authority transaction.
use crate::apps::identity::models::{
	AuthorizationBundle, AuthorizationCatalog, AuthorizationCatalogHistory,
};
use crate::apps::identity::repositories::catalog_mutation::NativeMutation;
use crate::apps::identity::repositories::{catalog_approvals, nodes};
use aidash_application::Result;
use aidash_application::authorization::catalog::carry_over::{CarryScope, Source};
use aidash_application::ports::catalog::CatalogMutation;
use aidash_domain::identity::catalog::{ApprovalScope, CatalogRestriction, RestrictionChange};
use aidash_domain::nodes::{DeploymentId, NodeAddress};
use aidash_domain::registry::{EntityRef, Entry};
use async_trait::async_trait;
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::orm::Model;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};

pub(crate) struct TenantCarry<'a> {
	pub tx: &'a mut crate::database::native::Transaction,
	pub snapshot: &'a mut aidash_application::authorization::Snapshot,
	pub tenant: &'a str,
	pub deployment: &'a DeploymentId,
}

/// Read under the shared policy lease; an approval writer cannot create a
/// source until that lease is released.
pub(crate) async fn has_source(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	entry: &Entry,
) -> Result<bool> {
	let rows = AuthorizationCatalog::objects()
		.filter(AuthorizationCatalog::field_tenant().eq(tenant.to_owned()))
		.filter(AuthorizationCatalog::field_entry_key().eq(entry.id.clone()))
		.filter(AuthorizationCatalog::field_enabled().eq(true))
		.filter(AuthorizationCatalog::field_carry_over().eq(true))
		.all_with_executor(&mut **tx)
		.await
		.map_err(FrameworkError::from)
		.map_err(crate::Error::from)?;
	let candidates: Vec<_> = rows
		.iter()
		.map(|row| aidash_domain::catalog::carry_over::CarryCandidate {
			version: &row.entry_version,
			enabled: true,
			carry_over: true,
		})
		.collect();
	Ok(
		aidash_domain::catalog::carry_over::source_version(&entry.version, &candidates)
			.ok()
			.flatten()
			.is_some(),
	)
}

impl TenantCarry<'_> {
	async fn rows(&mut self, node: &NodeAddress, id: &str) -> Result<Vec<AuthorizationCatalog>> {
		Ok(AuthorizationCatalog::objects()
			.filter(AuthorizationCatalog::field_tenant().eq(self.tenant.to_owned()))
			.filter(AuthorizationCatalog::field_node().eq(node.node.as_str().to_owned()))
			.filter(AuthorizationCatalog::field_entry_key().eq(id.to_owned()))
			.all_with_executor(&mut **self.tx)
			.await
			.map_err(FrameworkError::from)
			.map_err(crate::Error::from)?)
	}
}

/// Keep a failed carry attempt from aborting registration or leaving partial policy changes.
/// A broken connection still prevents the enclosing transaction from committing.
pub(crate) async fn carry_best_effort(
	scope: &mut TenantCarry<'_>,
	entry: &Entry,
) -> Result<Vec<aidash_application::authorization::catalog::carry_over::CarryResult>> {
	// Without a source nothing can carry, and the Node inventory lock is left
	// alone: a caller holding the policy only shared must not take it out of
	// the policy-then-Nodes order other work follows.
	if !has_source(&mut *scope.tx, scope.tenant, entry).await? {
		return Ok(nodes::list_live(&mut **scope.tx, scope.tenant)
			.await?
			.into_iter()
			.map(
				|node| aidash_application::authorization::catalog::carry_over::CarryResult {
					node: scope.deployment.node(node.name),
					source_version: None,
					changed: vec!["no earlier carrying approval".into()],
					unapproved_dependencies: vec![],
					carried: false,
				},
			)
			.collect());
	}
	let snapshot = scope.snapshot.clone();
	scope
		.tx
		.savepoint("approval_carry")
		.await
		.map_err(crate::Error::from)?;
	match aidash_application::authorization::catalog::carry_over::carry(scope, entry).await {
		Ok(results) => {
			scope
				.tx
				.release_savepoint("approval_carry")
				.await
				.map_err(crate::Error::from)?;
			Ok(results)
		}
		Err(error) => {
			scope
				.tx
				.rollback_to_savepoint("approval_carry")
				.await
				.map_err(crate::Error::from)?;
			scope
				.tx
				.release_savepoint("approval_carry")
				.await
				.map_err(crate::Error::from)?;
			*scope.snapshot = snapshot;
			tracing::warn!(%error, id = %entry.id, version = %entry.version, "approval carry failed");
			let nodes = nodes::list_live(&mut **scope.tx, scope.tenant).await?;
			Ok(nodes
				.into_iter()
				.map(
					|node| aidash_application::authorization::catalog::carry_over::CarryResult {
						node: scope.deployment.node(node.name),
						source_version: None,
						changed: vec![format!("approval carry failed: {error}")],
						unapproved_dependencies: vec![],
						carried: false,
					},
				)
				.collect())
		}
	}
}

#[async_trait]
impl CarryScope for TenantCarry<'_> {
	async fn nodes(&mut self) -> Result<Vec<NodeAddress>> {
		let tenant = self.tenant.to_owned();
		nodes::lock_tenant(&mut **self.tx, &tenant).await?;
		Ok(nodes::list_live(&mut **self.tx, &tenant)
			.await?
			.into_iter()
			.map(|n| self.deployment.node(n.name))
			.collect())
	}
	async fn sources(&mut self, node: &NodeAddress, id: &str) -> Result<Vec<Source>> {
		let mut result = Vec::new();
		for row in self
			.rows(node, id)
			.await?
			.into_iter()
			.filter(|r| r.enabled && r.carry_over)
		{
			let definition = catalog_approvals::definition(
				self.tx,
				&EntityRef {
					id: id.into(),
					version: row.entry_version.clone(),
				},
			)
			.await?;
			let history = AuthorizationCatalogHistory::objects()
				.filter(AuthorizationCatalogHistory::field_tenant().eq(row.tenant.clone()))
				.filter(AuthorizationCatalogHistory::field_node().eq(row.node.clone()))
				.filter(AuthorizationCatalogHistory::field_entry_key().eq(row.entry_key.clone()))
				.filter(
					AuthorizationCatalogHistory::field_entry_version()
						.eq(row.entry_version.clone()),
				)
				.filter(AuthorizationCatalogHistory::field_revision().eq(row.revision))
				.all_with_executor(&mut **self.tx)
				.await
				.map_err(FrameworkError::from)
				.map_err(crate::Error::from)?
				.pop()
				.ok_or_else(|| {
					aidash_application::Error::Conflict("approval history is missing".into())
				})?;
			let approver = if let Some(approver) = row.carried_from_approver {
				approver
			} else if history.actor != "carry-over" {
				history.actor
			} else {
				AuthorizationCatalogHistory::objects()
					.filter(AuthorizationCatalogHistory::field_tenant().eq(row.tenant.clone()))
					.filter(AuthorizationCatalogHistory::field_node().eq(row.node.clone()))
					.filter(
						AuthorizationCatalogHistory::field_entry_key().eq(row.entry_key.clone()),
					)
					.filter(
						AuthorizationCatalogHistory::field_entry_version()
							.eq(row.entry_version.clone()),
					)
					.order_by(&["-revision"])
					.all_with_executor(&mut **self.tx)
					.await
					.map_err(FrameworkError::from)
					.map_err(crate::Error::from)?
					.into_iter()
					.find(|revision| revision.actor != "carry-over")
					.map(|revision| revision.actor)
					.ok_or_else(|| {
						aidash_application::Error::Conflict("original approver is missing".into())
					})?
			};
			result.push(Source {
				definition,
				enabled: true,
				carry_over: true,
				scope: match row.scope.as_str() {
					"deployment" => ApprovalScope::Deployment,
					"node" => ApprovalScope::Node,
					_ => {
						return Err(aidash_application::Error::Conflict(
							"invalid approval scope".into(),
						));
					}
				},
				approver,
			});
		}
		Ok(result)
	}
	async fn approved(&mut self, node: &NodeAddress, dependency: &EntityRef) -> Result<bool> {
		let tenant = self.tenant.to_owned();
		catalog_approvals::approved(self.tx, &tenant, &node.node, dependency).await
	}
	async fn write_carried(
		&mut self,
		node: &NodeAddress,
		source: &Source,
		entry: &Entry,
		scope: ApprovalScope,
	) -> Result<()> {
		let tenant = self.tenant.to_owned();
		let rows = self.rows(node, &entry.id).await?;
		if rows.iter().any(|r| r.entry_version == entry.version) {
			return Err(aidash_application::Error::Conflict(
				"target approval already exists".into(),
			));
		}
		let row = rows
			.into_iter()
			.find(|r| r.entry_version == source.definition.version)
			.ok_or_else(|| {
				aidash_application::Error::Conflict("source approval disappeared".into())
			})?;
		let restriction = row
			.restriction
			.map(|r| serde_json::from_value::<CatalogRestriction>(r.0))
			.transpose()?;
		let reference = EntityRef {
			id: entry.id.clone(),
			version: entry.version.clone(),
		};
		let mut mutation = NativeMutation(self.tx);
		let binding = mutation
			.compare_and_set(
				&tenant,
				&node.node,
				&reference,
				0,
				true,
				RestrictionChange::Replace(restriction),
			)
			.await?
			.ok_or_else(|| aidash_application::Error::Conflict("target approval changed".into()))?;
		mutation.history(&binding, "carry-over", None).await?;
		for table in ["authorization_catalog", "authorization_catalog_history"] {
			let sql = Query::update()
				.table(Alias::new(table))
				.value(Alias::new("carry_over"), true)
				.value(
					Alias::new("scope"),
					if scope == ApprovalScope::Deployment {
						"deployment"
					} else {
						"node"
					},
				)
				.value(
					Alias::new("carried_from_version"),
					Some(source.definition.version.clone()),
				)
				.value(
					Alias::new("carried_from_approver"),
					Some(source.approver.clone()),
				)
				.and_where(Expr::col(Alias::new("tenant")).eq(tenant.clone()))
				.and_where(Expr::col(Alias::new("node")).eq(node.node.as_str()))
				.and_where(Expr::col(Alias::new("entry_id")).eq(entry.id.clone()))
				.and_where(Expr::col(Alias::new("entry_version")).eq(entry.version.clone()))
				.and_where(Expr::col(Alias::new("revision")).eq(binding.revision))
				.to_string(PostgresQueryBuilder);
			crate::database::native::query(&sql)
				.execute(&mut **self.tx)
				.await?;
		}
		if entry.kind == "agent" {
			let old = aidash_domain::qualified_agent(node, &entry.id, &source.definition.version);
			let new = aidash_domain::qualified_agent(node, &entry.id, &entry.version);
			let source_subject = self
				.snapshot
				.bundle
				.subjects
				.get(&old)
				.cloned()
				.ok_or_else(|| {
					aidash_application::Error::Conflict("source agent subject missing".into())
				})?;
			if source_subject.kind != aidash_domain::policy::SubjectKind::Agent
				|| source_subject.node.as_ref() != Some(&node.node)
			{
				return Err(aidash_application::Error::Conflict(
					"invalid source agent subject".into(),
				));
			}
			let subject = aidash_domain::policy::Subject {
				kind: aidash_domain::policy::SubjectKind::Agent,
				node: Some(node.node.clone()),
				groups: source_subject.groups,
				roles: Default::default(),
				attributes: serde_json::json!({}),
				enabled: source_subject.enabled,
				delegated_by: None,
			};
			if self
				.snapshot
				.bundle
				.subjects
				.insert(new.clone(), subject)
				.is_some()
			{
				return Err(aidash_application::Error::Conflict(
					"target agent subject already exists".into(),
				));
			}
			self.snapshot.bundle.validate()?;
			self.snapshot.revision = AuthorizationBundle::replace(
				&mut **self.tx,
				&tenant,
				self.snapshot.revision,
				serde_json::to_value(&self.snapshot.bundle)?,
				"carry-over",
				None,
			)
			.await?;
			let rows = AuthorizationCatalog::objects()
				.filter(AuthorizationCatalog::field_tenant().eq(tenant.clone()))
				.filter(AuthorizationCatalog::field_node().eq(node.node.as_str().to_owned()))
				.all_with_executor(&mut **self.tx)
				.await
				.map_err(FrameworkError::from)
				.map_err(crate::Error::from)?;
			for row in rows {
				let Some(value) = row.restriction else {
					continue;
				};
				let mut restriction: CatalogRestriction = serde_json::from_value(value.0)?;
				if !restriction.subjects.contains(&old) || !restriction.subjects.insert(new.clone())
				{
					continue;
				}
				restriction.validate()?;
				let reference = EntityRef {
					id: row.entry_key.clone(),
					version: row.entry_version.clone(),
				};
				let mut mutation = NativeMutation(self.tx);
				let binding = mutation
					.compare_and_set(
						&tenant,
						&node.node,
						&reference,
						row.revision,
						row.enabled,
						RestrictionChange::Replace(Some(restriction)),
					)
					.await?
					.ok_or_else(|| {
						aidash_application::Error::Conflict("restriction changed".into())
					})?;
				mutation.history(&binding, "carry-over", None).await?;
			}
		}
		Ok(())
	}
}
