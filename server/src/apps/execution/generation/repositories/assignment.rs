//! Native admission queries retain locks, bind values and the caller's transaction.
use crate::{
	Error,
	authorization::{access::Access, catalog, execution, identity::SubjectIdentity},
	federation::Federation,
};
use aidash_application::ports::generation::assignment::{
	Creation, GenerationAssignmentScope, GenerationAssignmentSession, GenerationAssignments,
	GenerationCreationScope,
};
use aidash_domain::{
	Task,
	federation::Delegation,
	generation::{
		policy::Policy,
		requests::{Assignment, Request},
	},
	policy::{PolicyBundle, Resource},
	registry::{EntityRef, Entry, Search},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{Expr, QueryStatementBuilder, SimpleExpr};
use serde_json::{Value, json};
use uuid::Uuid;
pub(crate) struct NativeAssignment<'a> {
	pub runtime: &'a Federation,
	pub access: &'a mut Access,
}
#[async_trait]
impl GenerationCreationScope for NativeAssignment<'_> {
	fn tenant(&self) -> &str {
		&self.access.identity.tenant
	}
	fn subject(&self) -> &str {
		&self.access.identity.subject
	}
	fn subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn bundle(&self) -> &PolicyBundle {
		&self.access.snapshot.bundle
	}
	fn node_id(&self) -> &str {
		&self.runtime.config.node_id
	}
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	fn request_id(&self) -> Uuid {
		Uuid::new_v4()
	}
	async fn catalog_entry(
		&mut self,
		reference: &EntityRef,
		action: &str,
	) -> aidash_application::Result<Entry> {
		catalog::entry(self.access, reference, action)
			.await
			.map_err(Into::into)
	}

	async fn previous_depth(&mut self) -> aidash_application::Result<Option<i32>> {
		let access = &mut *self.access;
		let f = self.runtime;
		let previous_depth: Option<i32> = {
			let query_bind_1 = &access.identity.tenant;
			let query_bind_2 = &f.config.node_id;
			let query_bind_3 = &access.subjects;
			crate::database::native::query_scalar(&reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("MAX(depth)"))
			.from(reinhardt::query::Alias::new("generation_requests"))
			.and_where(SimpleExpr::CustomWithExpr("(tenant = ? AND (? || '/agents/' || agent_id || '@' || agent_version) = ANY(?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), crate::database::text_array(query_bind_3.to_owned())]))
			.to_string(reinhardt::query::PostgresQueryBuilder))
	.scalar_one(&mut **access.tx)
	.await?
		};
		Ok(previous_depth)
	}

	async fn active(&mut self, policy_id: &str) -> aidash_application::Result<i64> {
		let access = &mut *self.access;
		let active: i64 = {
			let query_bind_1 = &access.identity.tenant;
			let query_bind_2 = policy_id;
			crate::database::native::query_scalar(&reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("COUNT(*)"))
			.from(reinhardt::query::Alias::new("generation_requests"))
			.and_where(SimpleExpr::CustomWithExpr("(tenant = ? AND policy_id = ? AND status IN ('PENDING_APPROVAL', 'QUEUED', 'ACTIVE'))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
			.to_string(reinhardt::query::PostgresQueryBuilder))
	.scalar_one(&mut **access.tx)
	.await?
		};
		Ok(active)
	}

	async fn insert(&mut self, creation: &Creation<'_>) -> aidash_application::Result<Request> {
		let access = &mut *self.access;
		let Creation {
			id,
			task,
			policy,
			definition,
			status,
			reason,
			depth,
			..
		} = creation;
		let id = *id;
		let task_id = task.id;
		let policy_id = policy.id.as_str();
		let limits = &policy.spec.limits;
		let status = *status;
		let reason = *reason;
		let depth = *depth;
		let generated: Request = {
			let query_bind_1 = id;
			let query_bind_2 = &access.identity.tenant;
			let query_bind_3 = policy_id;
			let query_bind_4 = policy.revision;
			let query_bind_5 = task_id;
			let query_bind_6 = task.workspace_id;
			let query_bind_7 = access.identity.credential_id;
			let query_bind_8 = &access.identity.subject;
			let query_bind_9 = &access.subjects;
			let query_bind_10 = &definition.id;
			let query_bind_11 = &definition.version;
			let query_bind_12 = json!(definition);
			let query_bind_13 = status;
			let query_bind_14 = reason;
			let query_bind_15 = depth;
			let query_bind_16 = limits.tokens_per_agent;
			let query_bind_17 = creation.lifetime_seconds;
			let query_bind_18 = creation.home_node;
			let query_bind_19 = creation.foreign_intent.clone();
			crate::database::query_as(
				&reinhardt::query::Query::insert()
					.into_table(reinhardt::query::Alias::new("generation_requests"))
					.columns([
						reinhardt::query::Alias::new("id"),
						reinhardt::query::Alias::new("tenant"),
						reinhardt::query::Alias::new("policy_id"),
						reinhardt::query::Alias::new("policy_revision"),
						reinhardt::query::Alias::new("task_id"),
						reinhardt::query::Alias::new("workspace_id"),
						reinhardt::query::Alias::new("credential_id"),
						reinhardt::query::Alias::new("root_subject"),
						reinhardt::query::Alias::new("subject_chain"),
						reinhardt::query::Alias::new("agent_id"),
						reinhardt::query::Alias::new("agent_version"),
						reinhardt::query::Alias::new("definition"),
						reinhardt::query::Alias::new("status"),
						reinhardt::query::Alias::new("reason"),
						reinhardt::query::Alias::new("depth"),
						reinhardt::query::Alias::new("token_limit"),
						reinhardt::query::Alias::new("expires_at"),
						reinhardt::query::Alias::new("home_node"),
						reinhardt::query::Alias::new("foreign_intent"),
					])
					.from_subquery(
						reinhardt::query::Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_5.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_6.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_7.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_8.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![crate::database::text_array(query_bind_9.to_owned())],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_10.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_11.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_12.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_13.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_14.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_15.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_16.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(CLOCK_TIMESTAMP() + MAKE_INTERVAL(secs => ?))".to_owned(),
								vec![Expr::value(query_bind_17.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_18.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_19.to_owned()).into()],
							))
							.to_owned(),
					)
					.returning_all()
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **access.tx)
			.await?
		};
		Ok(generated)
	}

	async fn allocate(
		&mut self,
		policy: &Policy,
		compaction_calls: i64,
		embedding_calls: i64,
	) -> aidash_application::Result<()> {
		let access = &mut *self.access;
		let policy_id = policy.id.as_str();
		let limits = &policy.spec.limits;

		let query_bind_1 = &access.identity.tenant;
		let query_bind_2 = policy_id;
		let query_bind_3 = limits.tokens_per_agent;
		let query_bind_4 = compaction_calls;
		let query_bind_5 = embedding_calls;
		crate::database::native::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("generation_policies"))
				.value_expr(
					reinhardt::query::Alias::new("generated_count"),
					reinhardt::query::Expr::cust("generated_count + 1"),
				)
				.value_expr(
					reinhardt::query::Alias::new("allocated_tokens"),
					SimpleExpr::CustomWithExpr(
						"(allocated_tokens + ?)".to_owned(),
						vec![Expr::value(query_bind_3.to_owned()).into()],
					),
				)
				.value_expr(
					reinhardt::query::Alias::new("allocated_compaction_calls"),
					SimpleExpr::CustomWithExpr(
						"(allocated_compaction_calls + ?)".to_owned(),
						vec![Expr::value(query_bind_4.to_owned()).into()],
					),
				)
				.value_expr(
					reinhardt::query::Alias::new("allocated_embedding_calls"),
					SimpleExpr::CustomWithExpr(
						"(allocated_embedding_calls + ?)".to_owned(),
						vec![Expr::value(query_bind_5.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(tenant = ? AND id = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?;
		Ok(())
	}

	async fn budget(
		&mut self,
		id: Uuid,
		policy: &Policy,
		compaction_calls: i64,
		embedding_calls: i64,
	) -> aidash_application::Result<()> {
		let access = &mut *self.access;
		let limits = &policy.spec.limits;

		let query_bind_1 = id;
		let query_bind_2 = limits.tokens_per_agent;
		let query_bind_3 = compaction_calls;
		let query_bind_4 = embedding_calls;
		crate::database::native::query(
			&reinhardt::query::Query::insert()
				.into_table(reinhardt::query::Alias::new("generation_budgets"))
				.columns([
					reinhardt::query::Alias::new("request_id"),
					reinhardt::query::Alias::new("token_limit"),
					reinhardt::query::Alias::new("compaction_call_limit"),
					reinhardt::query::Alias::new("embedding_call_limit"),
				])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						))
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?;
		Ok(())
	}

	async fn history(
		&mut self,
		id: Uuid,
		status: &str,
		reason: &str,
	) -> aidash_application::Result<()> {
		let access = &mut *self.access;

		let query_bind_1 = id;
		let query_bind_2 = status;
		let query_bind_3 = &access.identity.subject;
		let query_bind_4 = reason;
		crate::database::native::query(
			&reinhardt::query::Query::insert()
				.into_table(reinhardt::query::Alias::new("generation_history"))
				.columns([
					reinhardt::query::Alias::new("request_id"),
					reinhardt::query::Alias::new("status"),
					reinhardt::query::Alias::new("actor"),
					reinhardt::query::Alias::new("reason"),
				])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						))
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?;
		Ok(())
	}

	async fn event(
		&mut self,
		workspace: Uuid,
		kind: &str,
		data: Value,
	) -> aidash_application::Result<()> {
		self.runtime
			.store
			.event(&mut self.access.tx, Some(workspace), kind, data)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn visible(&mut self, job: &Request) -> aidash_application::Result<bool> {
		aidash_application::generation::visibility::visible(
			&mut crate::bootstrap::generation_visibility_scope(self.access),
			job,
		)
		.await
	}
}
#[async_trait]
impl GenerationAssignmentScope for NativeAssignment<'_> {
	fn context(&mut self, value: Value) {
		self.access.context = value;
	}
	fn replace_subjects(&mut self, subjects: Vec<String>) {
		self.access.subjects = subjects;
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}

	async fn task(&mut self, task_id: Uuid) -> aidash_application::Result<Task> {
		let access = &mut *self.access;
		let task: Task = {
			let query_bind_1 = task_id;
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
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_optional(&mut **access.tx)
			.await?
		}
		.ok_or(Error::Forbidden)?;
		Ok(task)
	}

	async fn workspace(&mut self, id: Uuid) -> aidash_application::Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn inherit_task_origin(&mut self, id: Uuid) -> aidash_application::Result<bool> {
		execution::inherit_task_origin(self.access, id)
			.await
			.map_err(Into::into)
	}
	async fn task_resource(&mut self, task: &Task) -> aidash_application::Result<Resource> {
		self.access.task_resource(task).await.map_err(Into::into)
	}
	async fn decide(
		&mut self,
		resource: &Resource,
		action: &str,
	) -> aidash_application::Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn policy(&mut self, id: &str) -> aidash_application::Result<Policy> {
		super::policy::load(&mut self.access.tx, &self.access.identity.tenant, id, true)
			.await
			.map_err(Into::into)
	}

	async fn existing(&mut self, task_id: Uuid) -> aidash_application::Result<Option<Request>> {
		let access = &mut *self.access;
		let existing: Option<Request> = {
			let query_bind_1 = task_id;
			crate::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
					))
					.from(reinhardt::query::Alias::new("generation_requests"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(task_id = ? AND home_node='')".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_optional(&mut **access.tx)
			.await?
		};
		Ok(existing)
	}

	async fn existing_run(
		&mut self,
		task_id: Uuid,
	) -> aidash_application::Result<Option<(String, String, Vec<String>)>> {
		let access = &mut *self.access;
		let existing: Option<(String, String, Vec<String>)> = {
			let query_bind_1 = task_id;
			let query_bind_2 = &access.identity.tenant;
			let query_bind_3 = &access.identity.subject;
			crate::database::native::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col((
							reinhardt::query::Alias::new("r"),
							reinhardt::query::Alias::new("agent_id"),
						)),
					))
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col((
							reinhardt::query::Alias::new("r"),
							reinhardt::query::Alias::new("agent_version"),
						)),
					))
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col((
							reinhardt::query::Alias::new("e"),
							reinhardt::query::Alias::new("subject_chain"),
						)),
					))
					.from_as(
						reinhardt::query::Alias::new("authorization_execution"),
						reinhardt::query::Alias::new("e"),
					)
					.join(
						reinhardt::query::JoinType::InnerJoin,
						reinhardt::query::TableRef::table_alias(
							reinhardt::query::Alias::new("runs"),
							reinhardt::query::Alias::new("r"),
						),
						reinhardt::query::Expr::cust("r.id = e.run_id"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(e.task_id = ? AND e.tenant = ? AND e.root_subject = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.columns(&["agent_id", "agent_version", "subject_chain"])
			.fetch_optional(&mut **access.tx)
			.await?
		};
		Ok(existing)
	}

	async fn catalog(&mut self, search: &Search) -> aidash_application::Result<Vec<Entry>> {
		catalog::list_in(self.access, search)
			.await
			.map_err(Into::into)
	}

	async fn generated(&mut self, entry: &Entry) -> aidash_application::Result<bool> {
		let access = &mut *self.access;
		let generated: bool = {
			let query_bind_1 = &entry.id;
			let query_bind_2 = &entry.version;
			crate::database::native::query_scalar(&reinhardt::query::Query::select()
				.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM generation_requests WHERE agent_id = ? AND agent_version = ?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.scalar_one(&mut **access.tx)
		.await?
		};
		Ok(generated)
	}

	async fn delegate(
		&mut self,
		task_id: Uuid,
		agent: &EntityRef,
	) -> aidash_application::Result<Delegation> {
		execution::delegate_in(self.runtime, self.access, task_id, agent)
			.await
			.map_err(Into::into)
	}
}

pub(crate) struct NativeAssignments {
	pub runtime: Federation,
	pub identity: SubjectIdentity,
}
struct Session {
	runtime: Federation,
	access: Access,
}
impl Session {
	fn scope(&mut self) -> NativeAssignment<'_> {
		NativeAssignment {
			runtime: &self.runtime,
			access: &mut self.access,
		}
	}
}
#[async_trait]
impl GenerationAssignments for NativeAssignments {
	async fn begin(&self) -> aidash_application::Result<Box<dyn GenerationAssignmentSession>> {
		Ok(Box::new(Session {
			runtime: self.runtime.clone(),
			access: Access::begin(&self.runtime.store, &self.identity).await?,
		}))
	}
	fn notify(&self) {
		self.runtime.notify.notify_waiters();
	}
}
#[async_trait]
impl GenerationAssignmentSession for Session {
	async fn finish(
		self: Box<Self>,
		result: aidash_application::Result<Assignment>,
	) -> aidash_application::Result<Assignment> {
		self.access
			.finish(result.map_err(Error::from))
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl GenerationCreationScope for Session {
	fn tenant(&self) -> &str {
		&self.access.identity.tenant
	}
	fn subject(&self) -> &str {
		&self.access.identity.subject
	}
	fn subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn bundle(&self) -> &PolicyBundle {
		&self.access.snapshot.bundle
	}
	fn node_id(&self) -> &str {
		&self.runtime.config.node_id
	}
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	fn request_id(&self) -> Uuid {
		Uuid::new_v4()
	}
	async fn catalog_entry(
		&mut self,
		reference: &EntityRef,
		action: &str,
	) -> aidash_application::Result<Entry> {
		self.scope().catalog_entry(reference, action).await
	}
	async fn previous_depth(&mut self) -> aidash_application::Result<Option<i32>> {
		self.scope().previous_depth().await
	}
	async fn active(&mut self, policy_id: &str) -> aidash_application::Result<i64> {
		self.scope().active(policy_id).await
	}
	async fn insert(&mut self, creation: &Creation<'_>) -> aidash_application::Result<Request> {
		self.scope().insert(creation).await
	}
	async fn allocate(
		&mut self,
		policy: &Policy,
		compaction_calls: i64,
		embedding_calls: i64,
	) -> aidash_application::Result<()> {
		self.scope()
			.allocate(policy, compaction_calls, embedding_calls)
			.await
	}
	async fn budget(
		&mut self,
		id: Uuid,
		policy: &Policy,
		compaction_calls: i64,
		embedding_calls: i64,
	) -> aidash_application::Result<()> {
		self.scope()
			.budget(id, policy, compaction_calls, embedding_calls)
			.await
	}
	async fn history(
		&mut self,
		id: Uuid,
		status: &str,
		reason: &str,
	) -> aidash_application::Result<()> {
		self.scope().history(id, status, reason).await
	}
	async fn event(
		&mut self,
		workspace: Uuid,
		kind: &str,
		data: Value,
	) -> aidash_application::Result<()> {
		self.scope().event(workspace, kind, data).await
	}
	async fn visible(&mut self, job: &Request) -> aidash_application::Result<bool> {
		self.scope().visible(job).await
	}
}
#[async_trait]
impl GenerationAssignmentScope for Session {
	fn context(&mut self, value: Value) {
		self.access.context = value;
	}
	fn replace_subjects(&mut self, subjects: Vec<String>) {
		self.access.subjects = subjects;
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn task(&mut self, id: Uuid) -> aidash_application::Result<Task> {
		self.scope().task(id).await
	}
	async fn workspace(&mut self, id: Uuid) -> aidash_application::Result<Resource> {
		self.scope().workspace(id).await
	}
	async fn inherit_task_origin(&mut self, id: Uuid) -> aidash_application::Result<bool> {
		self.scope().inherit_task_origin(id).await
	}
	async fn task_resource(&mut self, task: &Task) -> aidash_application::Result<Resource> {
		self.scope().task_resource(task).await
	}
	async fn decide(
		&mut self,
		resource: &Resource,
		action: &str,
	) -> aidash_application::Result<bool> {
		self.scope().decide(resource, action).await
	}
	async fn policy(&mut self, id: &str) -> aidash_application::Result<Policy> {
		self.scope().policy(id).await
	}
	async fn existing(&mut self, task_id: Uuid) -> aidash_application::Result<Option<Request>> {
		self.scope().existing(task_id).await
	}
	async fn existing_run(
		&mut self,
		task_id: Uuid,
	) -> aidash_application::Result<Option<(String, String, Vec<String>)>> {
		self.scope().existing_run(task_id).await
	}
	async fn catalog(&mut self, search: &Search) -> aidash_application::Result<Vec<Entry>> {
		self.scope().catalog(search).await
	}
	async fn generated(&mut self, entry: &Entry) -> aidash_application::Result<bool> {
		self.scope().generated(entry).await
	}
	async fn delegate(
		&mut self,
		task_id: Uuid,
		agent: &EntityRef,
	) -> aidash_application::Result<Delegation> {
		self.scope().delegate(task_id, agent).await
	}
}
