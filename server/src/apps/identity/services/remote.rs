//! Durable source authority for remote admission. A prepared grant is pinned to
//! a task revision and exact receiver definitions; possession never bypasses
//! current policy, credential, task, peer or receiver checks.
#[path = "remote/execution.rs"]
pub(crate) mod execution;

use super::{
	access::Access,
	execution::inherit_task_origin,
	identity::{Actor, SubjectIdentity},
	peer::execution::Inspection,
	policy::SubjectKind,
};
use crate::apps::identity::repositories::remote_grants::Grant;
use crate::{
	Error, Result,
	domain::{Task, qualified_agent},
	federation::{Federation, Peer},
	registry::{EntityRef, Search},
};
use reinhardt::injectable;
use reinhardt::query::Alias;
use reinhardt::query::ColumnRef;
use reinhardt::query::Expr;
use reinhardt::query::LockType;
use reinhardt::query::PostgresQueryBuilder;
use reinhardt::query::Query;
use reinhardt::query::SimpleExpr;

use chrono::{DateTime, Utc};
use reinhardt::query::QueryStatementBuilder as _;
use serde_json::{Value, json};

use uuid::Uuid;

// A receiver may describe only the requested Agent's exact direct dependencies.
// Never use peer-provided node IDs or arbitrary resource lists as authority.

async fn source_authority(
	access: &mut Access,
	task: &Task,
	node: &str,
	inspection: &Inspection,
) -> Result<()> {
	crate::generation::foreign::check_home(access, task, node, inspection.generation.as_ref())
		.await?;
	let executor = qualified_agent(node, &inspection.agent.id, &inspection.agent.version);
	if access.subjects.last() != Some(&executor)
		|| access
			.snapshot
			.bundle
			.subjects
			.get(&executor)
			.is_none_or(|subject| subject.kind != SubjectKind::Agent)
	{
		return Err(Error::Forbidden);
	}
	let workspace = access.workspace(task.workspace_id).await?;
	access.context = workspace.attributes.clone();
	access.require(&workspace, "workspace.read").await?;
	let resource = access.task_resource(task).await?;
	access.require(&resource, "task.read").await?;
	access.require(&resource, "task.delegate").await?;
	access.require(&resource, "task.execute").await?;
	let resource = access.resource("node", node, json!({"remote_node":node}));
	access.require(&resource, "federation.execute").await?;
	for definition in &inspection.definitions {
		let id = format!(
			"{node}/{}s/{}@{}",
			definition.kind, definition.entry.id, definition.entry.version
		);
		let mut resource = super::catalog::resource(access, &definition.metadata);
		resource.id = id;
		resource.attributes["remote_node"] = json!(node);
		resource.attributes["digest"] = json!(definition.digest);
		access.require(&resource, "registry.read").await?;
		let action = match definition.kind.as_str() {
			"agent" => "agent.execute",
			"model" => "model.infer",
			"tool" => "tool.invoke",
			"skill" => "skill.use",
			"cluster" => "cluster.execute",
			"compactor" => "compaction.invoke",
			_ => return Err(Error::Forbidden),
		};
		access.require(&resource, action).await?;
	}
	Ok(())
}
pub(crate) async fn live(access: &mut Access, id: Uuid) -> Result<bool> {
	Ok({
		let query_bind_1 = id;
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("NOT revoked AND expires_at > CLOCK_TIMESTAMP()"))
				.from(Alias::new("authorization_remote_grants"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut **access.tx)
		.await?
	})
}
async fn peer(access: &mut Access, node: &str) -> Result<()> {
	let peer: Option<Peer> = {
		let query_bind_1 = node;
		crate::database::query_as(
			&Query::select()
				.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
				.from(Alias::new("peers"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(node_id = ? AND enabled)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **access.tx)
		.await?
	};
	if peer.is_none_or(|p| p.protocol_version != crate::config::PROTOCOL_VERSION) {
		return Err(Error::Forbidden);
	}
	Ok(())
}
async fn inspect(
	f: &Federation,
	access: &mut Access,
	node: &str,
	agent: &EntityRef,
	requirements: &Search,
	task_id: Uuid,
	compactor: Option<&EntityRef>,
) -> Result<Inspection> {
	let resource = access.resource("node", node, json!({"remote_node":node}));
	access.require(&resource, "federation.execute").await?;
	peer(access, node).await?;
	let inspection: Inspection = super::peer::authority_request(f, node, "/scoped/execution/inspect", &json!({"tenant":access.identity.tenant,"subject":access.identity.subject,"task_id":task_id,"agent":agent,"requirements":requirements,"compactor":compactor})).await?;
	validate(&inspection, node, agent, requirements)?;
	if inspection.compactor.as_ref() != compactor {
		return Err(Error::Forbidden);
	}
	Ok(inspection)
}

// Only the destination peer may obtain the source-authorized task. Both this
// description and boolean verification share the exact live authority checks.

pub(crate) async fn description_lease(
	f: &Federation,
	node: &str,
	id: Uuid,
) -> Result<(Access, Description)> {
	// A scoped worker may commit a task revision between task_read and the
	// shared row lock in a read-only description. Revalidate from a new
	// authority snapshot rather than treating that transient race as a denial.
	// A revoked grant or changed policy fails without a retry.
	for attempt in 0..3 {
		let mut revision_race = false;
		match description_lease_mode(f, node, id, false, &mut revision_race).await {
			Err(Error::Forbidden) if revision_race && attempt < 2 => {
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
			result => return result,
		}
	}
	unreachable!("the last verification attempt returns")
}
async fn description_lease_mode(
	f: &Federation,
	node: &str,
	id: Uuid,
	command: bool,
	revision_race: &mut bool,
) -> Result<(Access, Description)> {
	let grant: Grant = {
		let query_bind_1 = id;
		let query_bind_2 = node;
		sqlx::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from(reinhardt::query::Alias::new("authorization_remote_grants"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ? AND node_id = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_optional(&f.store.pool)
		.await?
	}
	.ok_or(Error::Forbidden)?;
	let mut access = Access::begin(&f.store, &grant.identity())
		.await
		.map_err(|e| {
			if matches!(e, Error::Unauthorized) {
				Error::Forbidden
			} else {
				e
			}
		})?;
	let result = async {
		// Serialize command journals before acquiring task row locks. This also
		// prevents two shared leases upgrading to conflicting task writers.
		if command {
			{
				let query_bind_1 = id.to_string();
				sqlx::query(
					&reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(PG_ADVISORY_XACT_LOCK(HASHTEXTEXTENDED(?, 71003801)))".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.execute(&mut **access.tx)
				.await?
			};
		}

		let current: Grant = {
			let query_bind_1 = grant.id;
			sqlx::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
					))
					.from(reinhardt::query::Alias::new("authorization_remote_grants"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Share)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **access.tx)
			.await?
		};
		if !live(&mut access, current.id).await? {
			return Err(Error::Forbidden);
		}
		access.subjects = current.subject_chain.clone();
		let task = access.task_read(current.task_id).await?;
		let locked: Task = {
			let query_bind_1 = task.id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Share)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **access.tx)
			.await?
		};
		if locked.revision != task.revision {
			*revision_race = true;
			return Err(Error::Forbidden);
		}
		let execution = execution::binding(&mut access, current.id).await?;
		let expected_revision = execution
			.as_ref()
			.map_or(current.task_revision, |bound| bound.task_revision);
		if task.revision != expected_revision
			|| task.workspace_id != current.workspace_id
			|| (execution.is_none() && task.status != crate::domain::TaskStatus::Open)
		{
			return Err(Error::Forbidden);
		}
		// Commands may advance only this revision journal. The admission keeps
		// its original task image, so an unrelated source edit cannot substitute
		// input or silently change the exact receiver binding.
		let admitted_task = if let Some(bound) = &execution {
			let original: Task = serde_json::from_value(bound.initial_task.clone())?;
			if bound.grant_id != current.id
				|| bound.task_id != task.id
				|| original.id != task.id
				|| original.workspace_id != task.workspace_id
				|| original.revision != current.task_revision
			{
				return Err(Error::Forbidden);
			}
			original
		} else {
			task.clone()
		};
		let inspection: Inspection = serde_json::from_value(current.inspection)?;
		source_authority(&mut access, &task, node, &inspection).await?;
		access.remote_semantic_sources(current.id).await?;
		if !access.grant_reads_visible(current.id).await? {
			return Err(Error::Forbidden);
		}

		let agent = EntityRef {
			id: inspection.agent.id.clone(),
			version: inspection.agent.version.clone(),
		};
		let requirements = serde_json::from_value(task.requirements.clone())?;
		let semantic: crate::semantic::remote::Binding = serde_json::from_value(current.semantic)?;
		let request = semantic.request();
		let fresh = inspect(
			f,
			&mut access,
			node,
			&agent,
			&requirements,
			task.id,
			request.compactor(),
		)
		.await?;
		if !fresh.satisfies(&inspection) {
			return Err(Error::Forbidden);
		}
		if !live(&mut access, current.id).await? {
			return Err(Error::Forbidden);
		}
		if semantic::binding(f, &mut access, &task, node, &fresh, &request).await? != semantic {
			return Err(Error::RemoteSemantic(
				crate::semantic::remote::Failure::Configuration,
			));
		}
		Ok(Description {
			grant_id: current.id,
			source_node: f.config.node_id.clone(),
			target_node: current.node_id,
			source_tenant: current.tenant,
			source_subject: current.root_subject,
			task: admitted_task,
			inspection,
			expires_at: current.expires_at,
			semantic,
		})
	}
	.await;
	match result {
		Ok(description) => Ok((access, description)),
		Err(error) => access.finish(Err(error)).await,
	}
}

#[cfg(test)]
#[path = "../tests/services_remote_tests.rs"]
mod tests;

pub(crate) use crate::apps::identity::serializers::remote::{Description, VerifyInput};
pub use crate::apps::identity::serializers::remote::{PrepareInput, Prepared};

use http::HeaderMap;

#[derive(Clone)]
pub struct RemoteGrants {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_remote_grants(#[inject] runtime: Federation) -> RemoteGrants {
	RemoteGrants { runtime }
}

impl RemoteGrants {
	pub async fn prepare(
		&self,
		actor: Actor,
		task_id: Uuid,
		input: PrepareInput,
	) -> Result<Prepared> {
		let f = self.runtime.clone();
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		if input.id.is_nil()
			|| input.node_id == f.config.node_id
			|| !(1..=3600).contains(&input.ttl_seconds)
		{
			return Err(Error::Invalid(
				"invalid remote grant destination or lifetime".into(),
			));
		}
		crate::config::validate_node_id(&input.node_id)?;
		let mut access = Access::begin(&f.store, &identity).await?;
		let result = async {
        inherit_task_origin(&mut access,task_id).await?;
        let task = access.task_read(task_id).await?;
        if task.status!=crate::domain::TaskStatus::Open {return Err(Error::Conflict("task is already assigned".into()));}
        if access.subjects.len()>=32 {return Err(Error::Invalid("execution delegation depth exceeds 32".into()));}
        let executor = qualified_agent(&input.node_id,&input.agent.id,&input.agent.version);
        if access.snapshot.bundle.subjects.get(&executor).is_none_or(|s| s.kind!=SubjectKind::Agent) {return Err(Error::Forbidden);}
        access.subjects.push(executor);
        let workspace = access.workspace(task.workspace_id).await?;
        access.context=workspace.attributes.clone();
        let resource=access.task_resource(&task).await?;
        access.require(&resource,"task.delegate").await?;
        access.require(&resource,"task.execute").await?;
        let requirements: Search = serde_json::from_value(task.requirements.clone())?;
        let inspection=inspect(&f,&mut access,&input.node_id,&input.agent,&requirements,task.id,input.semantic.compactor()).await?;
        crate::generation::foreign::check_preparation(&task,inspection.generation.as_ref())?;
        let semantic=serde_json::to_value(semantic::binding(&f,&mut access,&task,&input.node_id,&inspection,&input.semantic).await?)?;
        source_authority(&mut access,&task,&input.node_id,&inspection).await?;
        let metadata=serde_json::to_value(&inspection)?;
        // Retain the task revision through persistence, after read authorization.
        let current: Task={ let query_bind_1 = task_id; aidash_server::database::query_as(&reinhardt::query::Query::select().expr(reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk))).from(reinhardt::query::Alias::new("tasks")).and_where(SimpleExpr::CustomWithExpr("(id = ?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()])).lock(reinhardt::query::LockType::Share).to_string(reinhardt::query::PostgresQueryBuilder)).fetch_one(&mut **access.tx).await? };
        if current.revision!=task.revision || current.status!=crate::domain::TaskStatus::Open {return Err(Error::Conflict("task changed during grant preparation".into()));}
        let inserted={ let query_bind_1 = input.id; let query_bind_2 = task.id; let query_bind_3 = task.revision; let query_bind_4 = task.workspace_id; let query_bind_5 = &input.node_id; let query_bind_6 = &identity.tenant; let query_bind_7 = identity.credential_id; let query_bind_8 = &identity.subject; let query_bind_9 = &access.subjects; let query_bind_10 = &metadata; let query_bind_11 = input.ttl_seconds as f64; sqlx::query(&format!("{} ON CONFLICT DO NOTHING", reinhardt::query::Query::insert().into_table(reinhardt::query::Alias::new("authorization_remote_grants")).columns([reinhardt::query::Alias::new("id"), reinhardt::query::Alias::new("task_id"), reinhardt::query::Alias::new("task_revision"), reinhardt::query::Alias::new("workspace_id"), reinhardt::query::Alias::new("node_id"), reinhardt::query::Alias::new("tenant"), reinhardt::query::Alias::new("credential_id"), reinhardt::query::Alias::new("root_subject"), reinhardt::query::Alias::new("subject_chain"), reinhardt::query::Alias::new("inspection"), reinhardt::query::Alias::new("expires_at")]).from_subquery(reinhardt::query::Query::select().expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_2.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_3.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_4.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_5.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_6.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_7.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_8.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![crate::database::text_array(query_bind_9.to_owned())])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_10.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(CLOCK_TIMESTAMP() + MAKE_INTERVAL(secs => ?))".to_owned(), vec![Expr::value(query_bind_11.to_owned()).into()])).to_owned()).to_owned().to_string(reinhardt::query::PostgresQueryBuilder))).execute(&mut **access.tx).await? }.rows_affected();
        if inserted==1 {
            { let query_bind_1 = input.id; let query_bind_2 = &semantic; sqlx::query(&reinhardt::query::Query::update().table(reinhardt::query::Alias::new("authorization_remote_grants")).value_expr(reinhardt::query::Alias::new("semantic"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_2.to_owned()).into()]))
                .and_where(SimpleExpr::CustomWithExpr("(id=?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()])).to_string(reinhardt::query::PostgresQueryBuilder)).execute(&mut **access.tx).await? };
        }
        let grant: Grant={ let query_bind_1 = input.id; sqlx::query_as(&reinhardt::query::Query::select().expr(reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk))).from(reinhardt::query::Alias::new("authorization_remote_grants")).and_where(SimpleExpr::CustomWithExpr("(id = ?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()])).lock(reinhardt::query::LockType::Share).to_string(reinhardt::query::PostgresQueryBuilder)).fetch_one(&mut **access.tx).await? };
        if grant.task_id != task_id
            || grant.task_revision != task.revision
            || grant.workspace_id != task.workspace_id
            || grant.node_id != input.node_id
            || grant.credential_id != identity.credential_id
            || grant.tenant != identity.tenant
            || grant.root_subject != identity.subject
            || grant.subject_chain != access.subjects
            || grant.inspection != metadata
            || grant.semantic != semantic
            || !live(&mut access, grant.id).await?
        {
            return Err(Error::Conflict("grant id already binds different or expired authority".into()));
        }
        if inserted==1 { f.store.event(&mut access.tx,Some(task.workspace_id),"task.remote_grant_prepared",json!({"grant_id":grant.id,"task_id":task_id,"node_id":grant.node_id,"expires_at":grant.expires_at})).await?; }
        grant.prepared()
    }.await;
		access.finish(result).await
	}
	pub(crate) async fn revoke(
		&self,
		actor: Actor,
		(task_id, id): (Uuid, Uuid),
	) -> Result<Prepared> {
		let f = self.runtime.clone();
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let mut access = Access::begin(&f.store, &identity).await?;
		let result = async {
			let task = access.task_read(task_id).await?;
			let resource = access.task_resource(&task).await?;
			access.require(&resource, "task.delegate").await?;
			let mut grant: Grant = {
				let query_bind_1 = id;
				let query_bind_2 = task_id;
				let query_bind_3 = &identity.tenant;
				let query_bind_4 = &identity.subject;
				sqlx::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
						.from(Alias::new("authorization_remote_grants"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ? AND task_id = ? AND tenant = ? AND root_subject = ?)"
								.to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
								Expr::value(query_bind_3.to_owned()).into(),
								Expr::value(query_bind_4.to_owned()).into(),
							],
						))
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			}
			.ok_or(Error::Forbidden)?;
			if !grant.revoked {
				{
					let query_bind_1 = id;
					sqlx::query(
						&Query::update()
							.table(Alias::new("authorization_remote_grants"))
							.value_expr(Alias::new("revoked"), Expr::cust("TRUE"))
							.and_where(SimpleExpr::CustomWithExpr(
								"(id = ?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.to_string(PostgresQueryBuilder),
					)
					.execute(&mut **access.tx)
					.await?
				};
				f.store
					.event(
						&mut access.tx,
						Some(task.workspace_id),
						"task.remote_grant_revoked",
						json!({"grant_id":id,"task_id":task_id}),
					)
					.await?;
				grant.revoked = true;
			}
			grant.prepared()
		}
		.await;
		access.finish(result).await
	}
	pub(crate) async fn describe(
		&self,
		headers: HeaderMap,
		input: VerifyInput,
	) -> Result<Description> {
		let f = self.runtime.clone();
		let (access, description) = description_lease(
			&f,
			crate::apps::identity::services::http_auth::peer_node(&headers)?,
			input.grant_id,
		)
		.await?;
		access.finish(Ok(description)).await
	}
	pub(crate) async fn verify(&self, headers: HeaderMap, input: VerifyInput) -> Result<bool> {
		let f = self.runtime.clone();
		let (access, _) = description_lease(
			&f,
			crate::apps::identity::services::http_auth::peer_node(&headers)?,
			input.grant_id,
		)
		.await?;
		access.finish(Ok(true)).await
	}
	pub(crate) async fn snapshot(
		&self,
		headers: HeaderMap,
		input: VerifyInput,
	) -> Result<crate::domain::WorkspaceSnapshot> {
		let f = self.runtime.clone();
		let (mut access, description) = description_lease(
			&f,
			crate::apps::identity::services::http_auth::peer_node(&headers)?,
			input.grant_id,
		)
		.await?;
		access.read_grant = Some(description.grant_id);
		let result = async {
			let snapshot = access
				.workspace_snapshot(description.task.workspace_id)
				.await?;
			if !live(&mut access, description.grant_id).await? {
				return Err(Error::Forbidden);
			}
			Ok(snapshot)
		}
		.await;
		access.finish(result).await
	}
}

#[path = "remote/operator.rs"]
pub(crate) mod operator;

#[path = "remote/reads.rs"]
pub(crate) mod reads;

#[path = "remote/semantic.rs"]
pub(crate) mod semantic;

fn validate(
	inspection: &Inspection,
	node: &str,
	agent: &EntityRef,
	requirements: &Search,
) -> Result<()> {
	aidash_application::federation::admission::validate_inspection(
		&crate::bootstrap::registry_validation(),
		inspection,
		node,
		agent,
		requirements,
	)
	.map_err(Into::into)
}
