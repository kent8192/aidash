//! Original Source statements preserve advisory locks, row leases, database TTL and audit writes.
use super::Grant as GrantRow;
use crate::apps::identity::repositories::home_execution::{Repository, Scope};
use crate::{
	Result as NativeResult,
	authorization::{access::Access, identity::SubjectIdentity},
	federation::Peer,
};
use aidash_application::{
	Result,
	ports::authorization::{
		home::{HomeRepository, HomeScope},
		source::{
			SourceAuthorityScope,
			grants::{GrantRepository, GrantScope},
		},
	},
};
use aidash_domain::{
	Task,
	federation::execution::{Inspection, PrepareInput, home::Grant},
	identity::execution::ExecutionPrincipal,
	policy::{PolicyBundle, Resource},
	semantic::remote::{Binding, Request},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef, Expr, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use uuid::Uuid;
pub(crate) async fn live(access: &mut Access, id: Uuid) -> NativeResult<bool> {
	Ok({
		let query_bind_1 = id;
		crate::database::native::query_scalar(
			&Query::select()
				.expr(Expr::cust("NOT revoked AND expires_at > CLOCK_TIMESTAMP()"))
				.from(Alias::new("authorization_remote_grants"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **access.tx)
		.await?
	})
}
#[async_trait]
impl SourceAuthorityScope for Scope {
	fn source_subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn source_bundle(&self) -> &PolicyBundle {
		&self.access.snapshot.bundle
	}
	fn source_context(&mut self, attributes: Value) {
		self.access.context = attributes;
	}
	fn source_resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn generation_home(
		&mut self,
		task: &Task,
		node: &str,
		generation: Option<&Value>,
	) -> Result<()> {
		crate::generation::foreign::check_home(&mut self.access, task, node, generation)
			.await
			.map_err(Into::into)
	}
	async fn source_workspace(&mut self, id: Uuid) -> Result<Resource> {
		HomeScope::workspace(self, id).await
	}
	async fn source_task_resource(&mut self, task: &Task) -> Result<Resource> {
		HomeScope::task_resource(self, task).await
	}
	async fn source_require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		HomeScope::require(self, resource, action).await
	}
}
#[async_trait]
impl GrantScope for Scope {
	fn replace_subjects(&mut self, subjects: Vec<String>) {
		self.access.subjects = subjects;
	}
	fn append_subject(&mut self, subject: String) {
		self.access.subjects.push(subject);
	}
	async fn inherit_task_origin(&mut self, id: Uuid) -> Result<()> {
		crate::authorization::execution::inherit_task_origin(&mut self.access, id)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn live(&mut self, id: Uuid) -> Result<bool> {
		live(&mut self.access, id).await.map_err(Into::into)
	}
	async fn grant_reads_visible(&mut self, id: Uuid) -> Result<bool> {
		self.access
			.grant_reads_visible(id)
			.await
			.map_err(Into::into)
	}
	async fn semantic_binding(
		&mut self,
		task: &Task,
		node: &str,
		inspection: &Inspection,
		request: &Request,
	) -> Result<Binding> {
		crate::authorization::remote::semantic::binding(
			&self.runtime,
			&mut self.access,
			task,
			node,
			inspection,
			request,
		)
		.await
		.map_err(Into::into)
	}
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()> {
		self.runtime
			.store
			.event(&mut self.access.tx, Some(workspace), kind, data)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn command_lock(&mut self, id: Uuid) -> Result<()> {
		let result: NativeResult<()> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = id.to_string();
				crate::database::native::query(
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
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn current_grant(&mut self, id: Uuid) -> Result<Grant> {
		let result: NativeResult<GrantRow> = async {
			let access = &mut *self.access;
			Ok({
				let query_bind_1 = id;
				crate::database::native::query_as(
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
			})
		}
		.await;
		result.map(Into::into).map_err(Into::into)
	}
	async fn locked_task(&mut self, id: Uuid) -> Result<Task> {
		let result: NativeResult<Task> = async {
			let access = &mut *self.access;
			Ok({
				let query_bind_1 = id;
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
			})
		}
		.await;
		result.map_err(Into::into)
	}

	async fn insert_grant(
		&mut self,
		input: &PrepareInput,
		task: &Task,
		metadata: &Value,
	) -> Result<u64> {
		let result: NativeResult<u64> = async {
			let identity = self.access.identity.clone();
			let access = &mut *self.access;
			Ok({
				let query_bind_1 = input.id;
				let query_bind_2 = task.id;
				let query_bind_3 = task.revision;
				let query_bind_4 = task.workspace_id;
				let query_bind_5 = &input.node_id;
				let query_bind_6 = &identity.tenant;
				let query_bind_7 = identity.credential_id;
				let query_bind_8 = &identity.subject;
				let query_bind_9 = &access.subjects;
				let query_bind_10 = metadata;
				let query_bind_11 = input.ttl_seconds as f64;
				crate::database::native::query(&format!(
					"{} ON CONFLICT DO NOTHING",
					reinhardt::query::Query::insert()
						.into_table(reinhardt::query::Alias::new("authorization_remote_grants"))
						.columns([
							reinhardt::query::Alias::new("id"),
							reinhardt::query::Alias::new("task_id"),
							reinhardt::query::Alias::new("task_revision"),
							reinhardt::query::Alias::new("workspace_id"),
							reinhardt::query::Alias::new("node_id"),
							reinhardt::query::Alias::new("tenant"),
							reinhardt::query::Alias::new("credential_id"),
							reinhardt::query::Alias::new("root_subject"),
							reinhardt::query::Alias::new("subject_chain"),
							reinhardt::query::Alias::new("inspection"),
							reinhardt::query::Alias::new("expires_at")
						])
						.from_subquery(
							reinhardt::query::Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()]
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()]
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()]
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_4.to_owned()).into()]
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_5.to_owned()).into()]
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_6.to_owned()).into()]
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_7.to_owned()).into()]
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_8.to_owned()).into()]
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![crate::database::text_array(query_bind_9.to_owned())]
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_10.to_owned()).into()]
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(CLOCK_TIMESTAMP() + MAKE_INTERVAL(secs => ?))".to_owned(),
									vec![Expr::value(query_bind_11.to_owned()).into()]
								))
								.to_owned()
						)
						.to_owned()
						.to_string(reinhardt::query::PostgresQueryBuilder)
				))
				.execute(&mut **access.tx)
				.await?
			}
			.rows_affected())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn persist_semantic(&mut self, id: Uuid, semantic: &Value) -> Result<()> {
		let result: NativeResult<()> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = id;
				let query_bind_2 = semantic;
				crate::database::native::query(
					&reinhardt::query::Query::update()
						.table(reinhardt::query::Alias::new("authorization_remote_grants"))
						.value_expr(
							reinhardt::query::Alias::new("semantic"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.execute(&mut **access.tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn revocation_grant(&mut self, task_id: Uuid, id: Uuid) -> Result<Option<Grant>> {
		let result: NativeResult<Option<GrantRow>> = async {
			let identity = self.access.identity.clone();
			let access = &mut *self.access;
			Ok({
				let query_bind_1 = id;
				let query_bind_2 = task_id;
				let query_bind_3 = &identity.tenant;
				let query_bind_4 = &identity.subject;
				crate::database::native::query_as(
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
			})
		}
		.await;
		result.map(|r| r.map(Into::into)).map_err(Into::into)
	}
	async fn revoke_locked(&mut self, id: Uuid) -> Result<()> {
		let result: NativeResult<()> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = id;
				crate::database::native::query(
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
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl GrantRepository for Repository {
	type Scope = Scope;
	fn source_node_id(&self) -> &str {
		&self.runtime.config.node_id
	}
	fn source_identity(&self) -> Option<ExecutionPrincipal> {
		HomeRepository::identity(self)
	}
	fn protocol_version(&self) -> &str {
		crate::config::PROTOCOL_VERSION
	}
	fn validation(&self) -> aidash_application::registry::DefinitionValidation {
		crate::bootstrap::registry_validation_for(&self.runtime.store)
	}
	async fn source_begin(&self) -> Result<Scope> {
		HomeRepository::begin(self).await
	}
	async fn begin_grant(&self, grant: &Grant) -> Result<Scope> {
		let identity = SubjectIdentity {
			http_session: None,
			tenant: grant.tenant.clone(),
			subject: grant.root_subject.clone(),
			credential_id: grant.credential_id,
		};
		Ok(Scope {
			runtime: self.runtime.clone(),
			access: Box::new(Access::begin(&self.runtime.store, &identity).await?),
		})
	}
	async fn source_grant(&self, id: Uuid, node: &str) -> Result<Option<Grant>> {
		let result: NativeResult<Option<GrantRow>> = async {
			let f = &self.runtime;
			Ok({
				let query_bind_1 = id;
				let query_bind_2 = node;
				crate::database::native::query_as(
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
			})
		}
		.await;
		result.map(|r| r.map(Into::into)).map_err(Into::into)
	}
	async fn source_request<T: DeserializeOwned + Send>(
		&self,
		node: &str,
		path: &str,
		input: &Value,
	) -> Result<T> {
		HomeRepository::request(self, node, path, input).await
	}
}

#[async_trait]
impl aidash_application::ports::authorization::source::SourcePeerScope for Scope {
	async fn peer(&mut self, node: &str) -> Result<Option<Peer>> {
		peer(&mut self.access, node).await.map_err(Into::into)
	}
}

pub(crate) async fn peer(access: &mut Access, node: &str) -> NativeResult<Option<Peer>> {
	Ok({
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
	})
}
