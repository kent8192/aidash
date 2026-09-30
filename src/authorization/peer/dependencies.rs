//! Bounded dependency traversal. A peer evaluates only its local graph and
//! returns the remaining node-qualified edges; it never calls another peer.
//! The initiating reader must have an explicit mapping at every authority.
use crate::{
	Error, Result,
	authorization::{access::Access, catalog},
	federation::{Federation, Peer},
	registry::{EntityRef, digest},
};
use axum::{Json, extract::State, http::HeaderMap};
use sea_orm::sea_query::{Alias, Asterisk, Expr, LockType, PostgresQueryBuilder, Query};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

const LIMIT: usize = 256;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Reference {
	Grant {
		node_id: String,
		execution_node: String,
		grant_id: Uuid,
		admission_id: Uuid,
	},
	Admission {
		node_id: String,
		home_node: String,
		grant_id: Uuid,
		admission_id: Uuid,
	},
	Registry {
		node_id: String,
		id: String,
		version: String,
		digest: String,
	},
}
impl Reference {
	fn node(&self) -> &str {
		match self {
			Self::Grant { node_id, .. }
			| Self::Admission { node_id, .. }
			| Self::Registry { node_id, .. } => node_id,
		}
	}
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
	tenant: String,
	subject: String,
	reference: Reference,
}
#[derive(Deserialize, Serialize)]
pub(crate) struct Checked {
	visible: bool,
	pending: Vec<Reference>,
}

pub(crate) async fn verify(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<Input>,
) -> Result<Json<Checked>> {
	if input.reference.node() != f.config.node_id {
		return Err(Error::Forbidden);
	}
	let node = crate::api::peer_node(&headers)?;
	let mut access = super::access(&f, node, &input.tenant, &input.subject).await?;
	access.dependency_frontier = Some(vec![]);
	let result = async {
		let visible = access.check_dependency(&input.reference).await?;
		let pending = access.dependency_frontier.take().unwrap_or_default();
		if pending.len() > LIMIT {
			return Ok(Json(Checked {
				visible: false,
				pending: vec![],
			}));
		}
		Ok(Json(Checked {
			visible,
			pending: if visible { pending } else { vec![] },
		}))
	}
	.await;
	access.finish(result).await
}

impl Access {
	pub(crate) async fn foreign_run_base_visible(
		&mut self,
		run: &crate::domain::Run,
	) -> Result<bool> {
		let record: Option<(Uuid, Vec<String>, serde_json::Value)> = sqlx::query_as(
			&Query::select()
				.columns(["credential_id", "subject_chain", "description"].map(Alias::new))
				.from(Alias::new("authorization_remote_admissions"))
				.and_where(Expr::cust(
					"id=$1 AND tenant=$2 AND source_node=$3 AND task_id=$4",
				))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.bind(run.id)
		.bind(&self.identity.tenant)
		.bind(&run.home_node)
		.bind(run.task_id)
		.fetch_optional(&mut **self.tx)
		.await?;
		let Some((credential_id, subjects, value)) = record else {
			return Ok(false);
		};
		let d: super::super::remote::Description = serde_json::from_value(value)?;
		if d.source_node != run.home_node
			|| d.target_node != self.node_id
			|| d.task.id != run.task_id
			|| d.task.workspace_id != run.workspace_id
			|| d.inspection.agent.id != run.agent_id
			|| d.inspection.agent.version != run.agent_version
			|| d.expires_at <= chrono::Utc::now()
		{
			return Ok(false);
		}
		let workspace = self.resource(
			"workspace",
			format!("{}/workspaces/{}", run.home_node, run.workspace_id),
			json!({}),
		);
		let task = self.resource(
			"task",
			format!("{}/tasks/{}", run.home_node, run.task_id),
			json!({"created_by":d.task.created_by,"requirements":d.task.requirements}),
		);
		let memory = self.resource("memory",&run.agent_id,json!({"created_by":crate::domain::qualified_agent(&self.node_id,&run.agent_id,&run.agent_version),"version":run.agent_version}));
		if !self.decide(&workspace, "workspace.read").await?
			|| !self.decide(&task, "task.read").await?
			|| !self
				.decide(&self.resource("run", run.id, json!({})), "run.read")
				.await? || !self.decide(&memory, "memory.read").await?
		{
			return Ok(false);
		}
		let mapped: Option<Uuid> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("credential_id"))
				.from(Alias::new("authorization_peer_mappings"))
				.and_where(Expr::cust(
					"source_node=$1 AND source_tenant=$2 AND source_subject=$3 AND tenant=$4 AND enabled",
				))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.bind(&run.home_node)
		.bind(&d.source_tenant)
		.bind(&d.source_subject)
		.bind(&self.identity.tenant)
		.fetch_optional(&mut **self.tx)
		.await?;
		if mapped != Some(credential_id) {
			return Ok(false);
		}
		let Some(root) = subjects.first() else {
			return Ok(false);
		};
		let identity = super::super::identity::SubjectIdentity {
			credential_id,
			tenant: self.identity.tenant.clone(),
			subject: root.clone(),
		};
		let snapshot = match identity.lock_with_mode(&mut self.tx, false).await {
			Ok(snapshot) => snapshot,
			Err(Error::Unauthorized | Error::Forbidden) => return Ok(false),
			Err(error) => return Err(error),
		};
		let viewer_snapshot = std::mem::replace(&mut self.snapshot, snapshot);
		let viewer_subjects = std::mem::replace(&mut self.subjects, subjects);
		let result = async {
			if !self.decide(&workspace, "workspace.read").await?
				|| !self.decide(&task, "task.read").await?
				|| !self.decide(&task, "task.execute").await?
				|| (!d.semantic.disabled() && !self.decide(&workspace, "semantic.use").await?)
			{
				return Ok(false);
			}
			for definition in &d.inspection.definitions {
				let entry = match catalog::entry(self, &definition.entry, "registry.read").await {
					Ok(entry) => entry,
					Err(Error::Forbidden | Error::NotFound(_)) => return Ok(false),
					Err(error) => return Err(error),
				};
				let action = match entry.kind.as_str() {
					"agent" => "agent.execute",
					"model" => "model.infer",
					"tool" => "tool.invoke",
					"skill" => "skill.use",
					"cluster" => "cluster.execute",
					"compactor" => "compaction.invoke",
					_ => return Ok(false),
				};
				if digest(&serde_json::to_value(&entry)?) != definition.digest
					|| !self
						.decide(&catalog::resource(self, &entry), action)
						.await?
				{
					return Ok(false);
				}
			}
			Ok(true)
		}
		.await;
		self.snapshot = viewer_snapshot;
		self.subjects = viewer_subjects;
		result
	}
	/// Append an edge even when the local graph has already visited a source.
	/// Deduplication happens at the coordinator, after each edge's authority is checked.
	pub(crate) async fn received_semantic_visible(&mut self, run: Uuid) -> Result<bool> {
		let bound: Option<(String, Uuid)> = sqlx::query_as(
			&Query::select()
				.columns(["source_node", "grant_id"].map(Alias::new))
				.from(Alias::new("authorization_remote_admissions"))
				.and_where(Expr::cust(
					"id=$1 AND tenant=$2 AND description->'semantic'->>'mode'='required_home'",
				))
				.to_string(PostgresQueryBuilder),
		)
		.bind(run)
		.bind(&self.identity.tenant)
		.fetch_optional(&mut **self.tx)
		.await?;
		let Some((node_id, grant_id)) = bound else {
			return Ok(true);
		};
		let reference = Reference::Grant {
			node_id,
			execution_node: self.node_id.clone(),
			grant_id,
			admission_id: run,
		};
		if let Some(pending) = &mut self.dependency_frontier {
			pending.push(reference);
			return Ok(pending.len() <= LIMIT);
		}
		self.verify_dependencies(vec![reference]).await
	}

	pub(crate) async fn check_dependency(&mut self, reference: &Reference) -> Result<bool> {
		if reference.node() != self.node_id {
			return Ok(false);
		}
		let result = match reference {
			Reference::Admission {
				home_node,
				grant_id,
				admission_id,
				..
			} => {
				let run: Option<crate::domain::Run> = sqlx::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("runs"))
						.and_where(Expr::cust("id=$1 AND home_node=$2"))
						.to_string(PostgresQueryBuilder),
				)
				.bind(admission_id)
				.bind(home_node)
				.fetch_optional(&mut **self.tx)
				.await?;
				let Some(run) = run else {
					return Ok(false);
				};
				let bound: Option<Uuid> = sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("grant_id"))
						.from(Alias::new("authorization_remote_admissions"))
						.and_where(Expr::cust("id=$1"))
						.to_string(PostgresQueryBuilder),
				)
				.bind(admission_id)
				.fetch_optional(&mut **self.tx)
				.await?;
				if bound != Some(*grant_id) {
					return Ok(false);
				}
				Box::pin(self.run_visible(&run)).await
			}
			Reference::Grant {
				execution_node,
				grant_id,
				admission_id,
				..
			} => {
				Box::pin(super::super::remote::reads::visible(
					self,
					execution_node,
					*grant_id,
					*admission_id,
				))
				.await
			}
			Reference::Registry {
				id,
				version,
				digest: expected,
				..
			} => {
				let node = self.resource("node", &self.node_id, json!({}));
				if !self.decide(&node, "federation.discover").await? {
					return Ok(false);
				}
				let reference = EntityRef {
					id: id.clone(),
					version: version.clone(),
				};
				let entry = catalog::entry(self, &reference, "registry.read").await?;
				Ok(entry.kind == "agent"
					&& digest(&serde_json::to_value(&entry)?) == *expected
					&& self
						.decide(&catalog::resource(self, &entry), "agent.execute")
						.await?)
			}
		};
		match result {
			Err(
				Error::Forbidden
				| Error::Unauthorized
				| Error::NotFound(_)
				| Error::RemoteSemantic(_),
			) => Ok(false),
			other => other,
		}
	}

	pub(crate) async fn verify_dependencies(
		&mut self,
		mut pending: Vec<Reference>,
	) -> Result<bool> {
		let mut visited = std::collections::BTreeSet::new();
		let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
		while let Some(reference) = pending.pop() {
			let remaining = deadline.saturating_duration_since(std::time::Instant::now());
			if remaining.is_zero() {
				return Ok(false);
			}
			let key = digest(&serde_json::to_value(&reference)?);
			if !visited.insert(key) {
				continue;
			}
			if visited.len() > LIMIT || pending.len() > LIMIT {
				return Ok(false);
			}
			let checked = if reference.node() == self.node_id {
				self.dependency_frontier = Some(vec![]);
				let visible = self.check_dependency(&reference).await?;
				Checked {
					visible,
					pending: self.dependency_frontier.take().unwrap_or_default(),
				}
			} else {
				let node = reference.node();
				let resource = self.resource("node", node, json!({"remote_node":node}));
				if !self.decide(&resource, "federation.discover").await? {
					return Ok(false);
				}
				let peer: Option<Peer> = sqlx::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("peers"))
						.and_where(Expr::cust("node_id=$1 AND enabled"))
						.lock(LockType::Share)
						.to_string(PostgresQueryBuilder),
				)
				.bind(node)
				.fetch_optional(&mut **self.tx)
				.await?;
				let Some(peer) = peer else {
					return Ok(false);
				};
				if peer.protocol_version != crate::config::PROTOCOL_VERSION {
					return Ok(false);
				}
				let Ok(token) = crate::config::peer_secret(&peer.credential_env) else {
					return Ok(false);
				};
				let reply = self
					.peer_client
					.post(format!(
						"{}/federation/v0.1/scoped/dependencies/verify",
						peer.endpoint.trim_end_matches('/')
					))
					.timeout(remaining.min(std::time::Duration::from_secs(10)))
					.bearer_auth(token)
					.header("x-aidash-node", &self.node_id)
					.header("x-aidash-protocol", crate::config::PROTOCOL_VERSION)
					.json(
						&json!({"tenant":self.identity.tenant,"subject":self.identity.subject,"reference":reference}),
					)
					.send()
					.await;
				let Ok(reply) = reply else {
					return Ok(false);
				};
				if !reply.status().is_success() {
					return Ok(false);
				}
				let Ok(checked) = crate::response::json::<Checked>(reply, 131072).await else {
					return Ok(false);
				};
				checked
			};
			if !checked.visible || checked.pending.len() > LIMIT {
				return Ok(false);
			}
			pending.extend(checked.pending);
		}
		Ok(true)
	}
}
