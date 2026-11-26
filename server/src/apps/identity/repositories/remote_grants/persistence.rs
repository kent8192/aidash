//! Original Source statements preserve advisory locks, row leases, database TTL and audit writes.
use super::Grant as GrantRow;
use crate::apps::identity::repositories::home_execution::{Repository, Scope};
use crate::{
	Result as NativeResult,
	authorization::{access::Access, identity::SubjectIdentity},
};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
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
	federation::{
		execution::{Inspection, PrepareInput, home::Grant},
		tool_lists::{FederationToolRequest, ToolList},
	},
	identity::execution::ExecutionPrincipal,
	nodes::NodeAddress,
	policy::{PolicyBundle, Resource},
	semantic::remote::{Binding, Request},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait as _, LockType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use uuid::Uuid;
pub(crate) async fn live(access: &mut Access, id: Uuid) -> NativeResult<bool> {
	Ok({
		let query_bind_1 = id;
		crate::database::native::query_scalar(
			&Query::select()
				.expr(super::live_condition("authorization_remote_grants"))
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
/// Saves the answer of an onward `delegate` (#654) as the replay of the
/// asking grant's command, so a repeat under the same key answers it after the
/// target has claimed the child. `execute` saves a local delegation's answer
/// inside the command's lease and moves the binding to the Task's revision;
/// this runs after the lease, when the hand-on has changed nothing of the
/// grant's own Task, so it leaves the binding alone. A repeat that raced this
/// one saved the same answer, so an existing row is kept.
pub(crate) async fn save_onward_replay(
	access: &mut Access,
	grant: Uuid,
	(key, digest): (&str, &str),
	result: &Value,
) -> NativeResult<()> {
	let (sql, values) = Query::insert()
		.into_table(Alias::new("authorization_remote_commands"))
		.columns(["grant_id", "request_key", "digest", "result", "tenant"].map(Alias::new))
		.values_panic::<_, reinhardt::query::Value>([
			grant.into(),
			key.to_owned().into(),
			digest.to_owned().into(),
			result.clone().into(),
			access.identity.tenant.clone().into(),
		])
		.on_conflict(
			OnConflict::columns(["grant_id", "request_key"].map(Alias::new))
				.do_nothing()
				.to_owned(),
		)
		.build(PostgresQueryBuilder);
	access
		.tx
		.execute(&sql, reinhardt::db::orm::execution::convert_values(values))
		.await?;
	Ok(())
}
/// The delegation chain a Remote Grant pins, read under the caller's lease:
/// its root person, every Agent so far, and the receiver's executor.
pub(crate) async fn subject_chain(access: &mut Access, id: Uuid) -> NativeResult<Vec<String>> {
	crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("subject_chain"))
			.from(Alias::new("authorization_remote_grants"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **access.tx)
	.await
}
#[async_trait]
impl SourceAuthorityScope for Scope {
	fn source_subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn source_replace_subjects(&mut self, subjects: Vec<String>) {
		self.access.subjects = subjects;
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
	async fn source_thread(&mut self, id: Uuid) -> Result<Resource> {
		HomeScope::thread(self, id).await
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
	fn home_address(&self) -> String {
		self.access.node_reference().to_owned()
	}
	async fn registered_tool_request(
		&mut self,
		receiver: &str,
	) -> Result<Option<FederationToolRequest>> {
		let (Ok(receiver), Ok(home)) = (
			NodeAddress::parse(receiver),
			NodeAddress::parse(self.access.node_reference()),
		) else {
			return Ok(None);
		};
		crate::apps::identity::repositories::node_registrations::get(
			self.access.tx.as_mut(),
			&self.access.identity.tenant,
			home.node.as_str(),
			&receiver,
		)
		.await
		.map(|registration| registration.map(|registration| registration.tool_request))
		.map_err(Into::into)
	}
	async fn run_only_tool_request(&mut self, grant: Uuid) -> Result<ToolList> {
		let stored: Option<Value> = crate::database::native::query_scalar(
			&Query::select()
				.expr(Expr::cust("COALESCE(tool_request_additions, '[]'::jsonb)"))
				.from(Alias::new("authorization_remote_grants"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(grant)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **self.access.tx)
		.await?;
		Ok(match stored {
			Some(value) => serde_json::from_value(value)?,
			None => ToolList::default(),
		})
	}
	fn replace_subjects(&mut self, subjects: Vec<String>) {
		self.access.subjects = subjects;
	}
	fn append_subject(&mut self, subject: String) {
		self.access.subjects.push(subject);
	}
	async fn record_refusal(
		&mut self,
		resource: &aidash_domain::policy::Resource,
		reason: &str,
	) -> Result<()> {
		let records = self
			.access
			.subjects
			.iter()
			.map(|subject| {
				let input = self
					.access
					.evaluation(subject, resource, "federation.execute");
				let mut decision = self.access.snapshot.bundle.evaluate(&input);
				decision.allowed = false;
				decision.reason = reason.into();
				decision.revision = self.access.snapshot.revision;
				(input, decision)
			})
			.collect::<Vec<_>>();
		self.access.record(&records).await.map_err(Into::into)
	}
	async fn inherit_task_origin(&mut self, id: Uuid) -> Result<()> {
		crate::apps::execution::repositories::node_cancellations::require_task_open(
			self.access.tx.as_mut(),
			id,
		)
		.await?;
		crate::authorization::execution::inherit_task_origin(&mut self.access, id)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn is_subnode_thread(&mut self, thread: Uuid) -> Result<bool> {
		use crate::apps::execution::models::subnodes::SubnodeRecord;
		use reinhardt::db::orm::Model;
		let records = SubnodeRecord::objects()
			.filter(SubnodeRecord::field_tenant().eq(self.access.identity.tenant.clone()))
			.filter(SubnodeRecord::field_thread_id().eq(thread))
			.all_with_executor(self.access.tx.native_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?;
		Ok(!records.is_empty())
	}
	async fn live(&mut self, id: Uuid) -> Result<bool> {
		crate::apps::execution::repositories::node_cancellations::require_grant_open(
			self.access.tx.as_mut(),
			id,
		)
		.await?;
		live(&mut self.access, id).await.map_err(Into::into)
	}
	fn set_dependency_checks(
		&mut self,
		checks: Option<aidash_application::authorization::source::grants::DependencyChecks>,
	) {
		self.access.grant_dependency_checks = checks;
	}
	async fn release_probe(self) -> Result<()> {
		(*self.access)
			.into_native()?
			.rollback()
			.await
			.map_err(Into::into)
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
	async fn event(&mut self, thread: Uuid, kind: &str, data: Value) -> Result<()> {
		self.runtime
			.store
			.event(&mut self.access.tx, Some(thread), kind, data)
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
			crate::authorization::execution::set_task_model(
				access,
				task.id,
				input.task_model.as_ref(),
			)
			.await?;
			Ok({
				let query_bind_1 = input.id;
				let query_bind_2 = task.id;
				let query_bind_3 = task.revision;
				let query_bind_4 = task.thread_id;
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
							reinhardt::query::Alias::new("thread_id"),
							reinhardt::query::Alias::new("node_id"),
							reinhardt::query::Alias::new("tenant"),
							reinhardt::query::Alias::new("credential_id"),
							reinhardt::query::Alias::new("root_subject"),
							reinhardt::query::Alias::new("subject_chain"),
							reinhardt::query::Alias::new("inspection"),
							reinhardt::query::Alias::new("expires_at"),
							reinhardt::query::Alias::new("node")
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
								// The Home Node issuing the grant is its Thread's.
								.expr(crate::database::scope::thread_node(query_bind_4))
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
		revoke_in(&mut **self.access.tx, &self.access.identity.tenant, id)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl GrantRepository for Repository {
	type Scope = Scope;
	fn peer_cache(
		&self,
	) -> std::sync::Arc<aidash_application::authorization::source::grants::GrantPeerCache> {
		self.runtime.store.grant_peer_cache.clone()
	}
	async fn check_dependency(
		&self,
		route: &aidash_domain::federation::route::Route,
		input: &aidash_domain::federation::dependencies::Input,
		remaining: std::time::Duration,
	) -> aidash_application::ports::federation::dependencies::Check {
		use aidash_application::ports::federation::dependencies::DependencyTransport as _;
		crate::apps::federation::remote::services::wire::Wire::of(&self.runtime)
			.check(route, input, remaining)
			.await
	}
	fn source_node_id(&self) -> &str {
		self.runtime.config.deployment.as_str()
	}
	fn source_identity(&self) -> Option<ExecutionPrincipal> {
		HomeRepository::identity(self)
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
			chain: None,
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
				// A remote worker names the grant, not its Tenant (#241).
				.fetch_optional(f.store.pool.in_scope(&TenancyScope::Deployment(
					DeploymentScopeReason::FederationInbound,
				)))
				.await?
			})
		}
		.await;
		result.map(|r| r.map(Into::into)).map_err(Into::into)
	}
	async fn source_request<T: DeserializeOwned + Send>(
		&self,
		home: &str,
		node: &str,
		path: &str,
		input: &Value,
		identity: &ExecutionPrincipal,
	) -> Result<T> {
		let sender = aidash_domain::federation::route::Sender {
			node: Some(aidash_domain::nodes::NodeAddress::parse(home)?.node),
			subject: Some(identity.subject.clone()),
			..crate::authorization::peer::node_sender(&identity.tenant)?
		};
		crate::authorization::peer::authority_request(&self.runtime, &sender, node, path, input)
			.await
			.map_err(Into::into)
	}
}

#[async_trait]
impl aidash_application::ports::authorization::source::SourcePeerScope for Scope {
	async fn route(
		&mut self,
		node: &str,
	) -> Result<Option<aidash_domain::federation::route::Route>> {
		self.access.route_to(node).await.map_err(Into::into)
	}
}

/// Revocation is irreversible and shares the grant row with Home command leases.
pub(crate) async fn revoke_in(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
	id: Uuid,
) -> NativeResult<()> {
	crate::database::native::query(
		&Query::update()
			.table(Alias::new("authorization_remote_grants"))
			.value(Alias::new("revoked"), true)
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
			.to_string(PostgresQueryBuilder),
	)
	.execute(tx)
	.await?;
	Ok(())
}

/// The lifecycle caller holds exclusive policy; retain the existing revocation
/// event in that same transaction and make duplicate cleanup idempotent.
pub(crate) async fn revoke_for_node_in(
	store: &crate::store::Store,
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
	id: Uuid,
) -> NativeResult<()> {
	let grant: GrantRow = crate::database::native::query_as(
		&Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("authorization_remote_grants"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("tenant").eq(tenant))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *tx)
	.await?;
	if !grant.revoked {
		revoke_in(tx, tenant, id).await?;
		store
			.event_in_executor(
				tx,
				Some(grant.thread_id),
				"task.remote_grant_revoked",
				serde_json::json!({"grant_id":id,"task_id":grant.task_id}),
			)
			.await?;
	}
	Ok(())
}
