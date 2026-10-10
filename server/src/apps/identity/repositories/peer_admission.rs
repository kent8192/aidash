//! ORM conversion and original native statements preserve receiver locks and idempotent writes.
use crate::{
	Result as NativeResult, authorization::access::Access, federation::Federation, store::Store,
};
use aidash_application::{
	Result,
	ports::authorization::peer::{
		AdmissionMessages, PeerAdmissionRecords, PeerAdmissionRepository, PeerAdmissionScope,
		PeerInspectionScope,
	},
};
use aidash_domain::{
	Run,
	federation::execution::{
		Description,
		admission::{InspectInput, Record as State, RemoteExecutionControl},
	},
	generation::remote::Ancestor,
	identity::execution::ExecutionPrincipal,
	policy::Resource,
	registry::{EntityRef, Entry},
	run_state::RunInspection,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, ColumnRef::Asterisk, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use std::{borrow::Borrow, ops::DerefMut};
use uuid::Uuid;
struct Record {
	id: Uuid,
	source_node: String,
	grant_id: Uuid,
	task_id: Uuid,
	tenant: String,
	credential_id: Uuid,
	subject_chain: Vec<String>,
	description: Value,
}
crate::native_record!(Record {
	id,
	source_node,
	grant_id,
	task_id,
	tenant,
	credential_id,
	subject_chain,
	description
});

impl From<Record> for State {
	fn from(r: Record) -> Self {
		Self {
			id: r.id,
			source_node: r.source_node,
			grant_id: r.grant_id,
			task_id: r.task_id,
			tenant: r.tenant,
			credential_id: r.credential_id,
			subject_chain: r.subject_chain,
			description: r.description,
		}
	}
}
impl From<crate::apps::federation::peer::models::AuthorizationRemoteAdmission> for State {
	fn from(r: crate::apps::federation::peer::models::AuthorizationRemoteAdmission) -> Self {
		Self {
			id: r.id,
			source_node: r.source_node,
			grant_id: r.grant_id,
			task_id: r.task_id,
			tenant: r.tenant,
			credential_id: r.credential_id,
			subject_chain: r.subject_chain,
			description: r.description.into_inner(),
		}
	}
}
pub(crate) struct Scope<A, F> {
	pub(crate) access: A,
	pub(crate) runtime: F,
}
pub(crate) type Owned = Scope<Box<Access>, Federation>;
pub(crate) type Borrowed<'a> = Scope<&'a mut Access, &'a Federation>;
pub(crate) struct Repository<'a> {
	pub(crate) runtime: &'a Federation,
}
pub(crate) struct Records<'a> {
	pub(crate) store: &'a Store,
}

#[async_trait]
impl<A, F> PeerInspectionScope for Scope<A, F>
where
	A: DerefMut<Target = Access> + Send,
	F: Borrow<Federation> + Send + Sync,
{
	fn node_id(&self) -> &str {
		&self.runtime.borrow().config.node_id
	}
	fn identity(&self) -> ExecutionPrincipal {
		let id = &self.access.identity;
		ExecutionPrincipal {
			tenant: id.tenant.clone(),
			subject: id.subject.clone(),
			credential_id: id.credential_id,
		}
	}
	fn bundle(&self) -> &aidash_domain::policy::PolicyBundle {
		&self.access.snapshot.bundle
	}
	fn subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn push_subject(&mut self, subject: String) {
		self.access.subjects.push(subject)
	}
	fn context(&mut self) -> &mut Value {
		&mut self.access.context
	}
	fn resource(&self, kind: &str, id: &str, attrs: Value) -> Resource {
		self.access.resource(kind, id, attrs)
	}
	async fn require(&mut self, r: &Resource, action: &str) -> Result<()> {
		self.access.require(r, action).await.map_err(Into::into)
	}
	async fn generation(&mut self, source: &str, input: &InspectInput) -> Result<Option<Value>> {
		crate::generation::foreign::inspect(&mut self.access, source, input.task_id, &input.agent)
			.await
			.map_err(Into::into)
	}
	async fn entry(&mut self, r: &EntityRef, action: &str) -> Result<Entry> {
		crate::authorization::catalog::entry(&mut self.access, r, action)
			.await
			.map_err(Into::into)
	}
	fn entry_resource(&self, entry: &Entry) -> Resource {
		crate::authorization::catalog::resource(&self.access, entry)
	}
	async fn active_installation(&mut self, entry: &Entry) -> Result<bool> {
		crate::marketplace::active(&mut self.access, entry)
			.await
			.map_err(Into::into)
	}
	async fn pinned_installation(&mut self, entry: &Entry) -> Result<()> {
		crate::marketplace::check_pinned(&mut self.access, entry)
			.await
			.map_err(Into::into)
	}
	async fn bindings(
		&mut self,
		entry: &aidash_domain::registry::Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		crate::apps::registry::repositories::bindings::authorized(
			&mut self.access,
			entry,
			true,
			self.runtime.borrow().store.provider_credentials.is_some(),
		)
		.await
		.map_err(Into::into)
	}

	async fn lineage(&mut self) -> Result<Vec<Ancestor>> {
		let f = self.runtime.borrow();
		crate::generation::remote::lineage(&mut self.access, &f.config.node_id)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl PeerAdmissionScope for Owned {
	async fn lock_admission(&mut self, source: &str, description: &Description) -> Result<()> {
		let result: NativeResult<()> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = format!("{source}:{}", description.task.id);
				crate::database::native::query(
					&reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(PG_ADVISORY_XACT_LOCK(HASHTEXTEXTENDED(?, 71003209)))".to_owned(),
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
	async fn legacy_conflict(
		&mut self,
		source: &str,
		description: &Description,
		grant: Uuid,
	) -> Result<bool> {
		let result:NativeResult<bool>=async {let access=&mut *self.access;let input=crate::apps::identity::serializers::peer_admission::Input {grant_id:grant};Ok({ let query_bind_1 = source; let query_bind_2 = description.task.id; let query_bind_3 = input.grant_id; crate::database::native::query_scalar(&reinhardt::query::Query::select()
				.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM runs r WHERE home_node = ? AND task_id = ? AND NOT EXISTS(SELECT 1 FROM authorization_remote_admissions a WHERE a.id = r.id AND a.source_node = r.home_node AND a.grant_id = ?)))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()]))
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.scalar_one(&mut **access.tx)
		.await? })}.await;
		result.map_err(Into::into)
	}
	async fn existing(&mut self, source: &str, grant: Uuid) -> Result<Option<Uuid>> {
		let result: NativeResult<Option<Uuid>> = async {
			let access = &mut *self.access;
			let input =
				crate::apps::identity::serializers::peer_admission::Input { grant_id: grant };
			Ok({
				let query_bind_1 = source;
				let query_bind_2 = input.grant_id;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("authorization_remote_admissions"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(source_node=? AND grant_id=?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn insert(
		&mut self,
		source: &str,
		grant: Uuid,
		description: &Description,
		proposed: Uuid,
	) -> Result<()> {
		let result: NativeResult<()> = async {
			let access = &mut *self.access;
			let input =
				crate::apps::identity::serializers::peer_admission::Input { grant_id: grant };
			{
				let query_bind_1 = proposed;
				let query_bind_2 = source;
				let query_bind_3 = input.grant_id;
				let query_bind_4 = description.task.id;
				let query_bind_5 = &access.identity.tenant;
				let query_bind_6 = access.identity.credential_id;
				let query_bind_7 = &access.subjects;
				let query_bind_8 = serde_json::to_value(description)?;
				crate::database::native::query(&format!(
					"{} ON CONFLICT DO NOTHING",
					reinhardt::query::Query::insert()
						.into_table(reinhardt::query::Alias::new(
							"authorization_remote_admissions",
						))
						.columns([
							reinhardt::query::Alias::new("id"),
							reinhardt::query::Alias::new("source_node"),
							reinhardt::query::Alias::new("grant_id"),
							reinhardt::query::Alias::new("task_id"),
							reinhardt::query::Alias::new("tenant"),
							reinhardt::query::Alias::new("credential_id"),
							reinhardt::query::Alias::new("subject_chain"),
							reinhardt::query::Alias::new("description"),
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
									vec![crate::database::text_array(query_bind_7.to_owned())]
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_8.to_owned()).into()]
								))
								.to_owned()
						)
						.to_owned()
						.to_string(reinhardt::query::PostgresQueryBuilder)
				))
				.execute(&mut **access.tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn admitted(&mut self, source: &str, grant: Uuid) -> Result<Option<State>> {
		let result: NativeResult<Option<State>> = async {
			let access = &mut *self.access;
			let input =
				crate::apps::identity::serializers::peer_admission::Input { grant_id: grant };
			let row: Option<Record> = {
				let query_bind_1 = source;
				let query_bind_2 = input.grant_id;
				crate::database::native::query_as(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
						))
						.from(reinhardt::query::Alias::new(
							"authorization_remote_admissions",
						))
						.and_where(SimpleExpr::CustomWithExpr(
							"(source_node = ? AND grant_id = ?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.lock(reinhardt::query::LockType::Share)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(row.map(Into::into))
		}
		.await;
		result.map_err(Into::into)
	}
	async fn admission_live(&mut self, expires: DateTime<Utc>) -> Result<bool> {
		let result: NativeResult<bool> = async {
			let access = &mut *self.access;
			let description_expires_at = expires;
			Ok({
				let query_bind_1 = description_expires_at;
				crate::database::native::query_scalar(
					&reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(CAST(? AS TIMESTAMPTZ) > CLOCK_TIMESTAMP())".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.scalar_one(&mut **access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn receiver_live(&mut self, expires: DateTime<Utc>) -> Result<bool> {
		let result: NativeResult<bool> = async {
			let access = &mut *self.access;
			let description_expires_at = expires;
			Ok({
				let query_bind_1 = description_expires_at;
				crate::database::native::query_scalar(
					&reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(CAST(? AS TIMESTAMPTZ) > CLOCK_TIMESTAMP())".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.scalar_one(&mut **access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn bind_foreign(&mut self, d: &Description, id: Uuid, activate: bool) -> Result<()> {
		crate::generation::foreign::bind(&self.runtime, &mut self.access, d, id, activate)
			.await
			.map_err(Into::into)
	}
	async fn required_record(&mut self, id: Uuid) -> Result<State> {
		let result: NativeResult<State> = async {
			let access = &mut *self.access;
			let run_id = id;
			let row: Record = {
				let query_bind_1 = run_id;
				crate::database::native::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("authorization_remote_admissions"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.lock(LockType::Share)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **access.tx)
				.await?
			};
			Ok(row.into())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn require_active(&mut self, d: &Description, id: Uuid) -> Result<()> {
		crate::generation::foreign::require_active(&mut self.access, d, id)
			.await
			.map_err(Into::into)
	}
	fn worker(&mut self, durable: bool) {
		if durable {
			self.access.durable_audit = true
		};
		self.access.worker()
	}
	async fn activation_record(&mut self, id: Uuid) -> Result<Option<State>> {
		let result: NativeResult<Option<State>> = async {
			let access = &mut *self.access;
			let row: Option<Record> = {
				let query_bind_1 = id;
				crate::database::native::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("authorization_remote_admissions"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.lock(LockType::Share)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(row.map(Into::into))
		}
		.await;
		result.map_err(Into::into)
	}
	async fn lock_activation(&mut self, source: &str, description: &Description) -> Result<()> {
		let result: NativeResult<()> = async {
			let access = &mut *self.access;
			let d = description;
			{
				let query_bind_1 = format!("{source}:{}", d.task.id);
				crate::database::native::query(
					&Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(PG_ADVISORY_XACT_LOCK(HASHTEXTEXTENDED(?, 71003209)))".to_owned(),
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
	async fn insert_run(
		&mut self,
		source: &str,
		id: Uuid,
		description: &Description,
	) -> Result<()> {
		let result: NativeResult<()> = async {
			let access = &mut *self.access;
			let d = description;
			let node = access.node_id.clone();
			let snapshot = d.inspection.binding_snapshot.clone();
			snapshot.validate()?;
			if snapshot.agent.registry_node != node || !snapshot.remote {
				return Err(crate::Error::Forbidden);
			}
			// The receiver executes the Run, so its own Cache Salt Key decides;
			// the scoped admission's mapped Tenant salts it.
			self.runtime.store.require_projection(&snapshot, true)?;
			let context = crate::context::Context {
				binding_snapshot: Some(Box::new(snapshot)),
				..Default::default()
			};
			crate::database::native::query(&format!(
				"{} ON CONFLICT DO NOTHING",
				Query::insert()
					.into_table(Alias::new("runs"))
					.columns(
						[
							"id",
							"task_id",
							"workspace_id",
							"home_node",
							"agent_id",
							"agent_version",
							"context",
						]
						.map(Alias::new),
					)
					.from_subquery(
						Query::select()
							.expr(Expr::cust("$1"))
							.expr(Expr::cust("$2"))
							.expr(Expr::cust("$3"))
							.expr(Expr::cust("$4"))
							.expr(Expr::cust("$5"))
							.expr(Expr::cust("$6"))
							.expr(Expr::cust("$7"))
							.to_owned()
					)
					.to_owned()
					.to_string(PostgresQueryBuilder)
			))
			.bind(id)
			.bind(d.task.id)
			.bind(d.task.workspace_id)
			.bind(source)
			.bind(&d.inspection.agent.id)
			.bind(&d.inspection.agent.version)
			.bind(serde_json::to_value(context)?)
			.execute(&mut **access.tx)
			.await?;
			super::provider_credentials::admit(
				&mut **access.tx,
				id,
				&access.identity.tenant,
				&d.inspection.binding_snapshot,
				self.runtime.store.provider_credentials.is_some(),
			)
			.await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn activation_run(&mut self, id: Uuid) -> Result<Option<Run>> {
		let result: NativeResult<Option<Run>> = async {
			let access = &mut *self.access;
			Ok({
				let query_bind_1 = id;
				aidash_server::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("runs"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn accept_message(
		self,
		id: Uuid,
		sender: &str,
		content: &str,
		key: &str,
		limit: usize,
	) -> Result<()> {
		let mut access = (*self.access).into_native()?;
		let result = self
			.runtime
			.store
			.accept_run_message_in(access.tx.as_mut(), id, sender, content, key, limit)
			.await;
		access.finish(result).await.map_err(Into::into)
	}
	async fn finish(self, result: Result<()>) -> Result<()> {
		(*self.access)
			.finish(result.map_err(Into::into))
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl PeerAdmissionRecords for Records<'_> {
	async fn record(&self, id: Uuid) -> Result<Option<State>> {
		let result: NativeResult<Option<State>> = async {
			let store = self.store;
			let run_id = id;
			let row: Option<Record> = {
				let query_bind_1 = run_id;
				crate::database::native::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("authorization_remote_admissions"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&store.pool)
				.await?
			};
			Ok(row.map(Into::into))
		}
		.await;
		result.map_err(Into::into)
	}
	async fn description(&self, id: Uuid) -> Result<Value> {
		let result: NativeResult<Value> = async {
			let f_store = self.store;
			let run_id = id;
			Ok({
				let query_bind_1 = run_id;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("description"))
						.from(Alias::new("authorization_remote_admissions"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_one(&f_store.pool)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl PeerAdmissionRecords for Repository<'_> {
	async fn record(&self, id: Uuid) -> Result<Option<State>> {
		Records {
			store: &self.runtime.store,
		}
		.record(id)
		.await
	}
	async fn description(&self, id: Uuid) -> Result<Value> {
		Records {
			store: &self.runtime.store,
		}
		.description(id)
		.await
	}
}
#[async_trait]
impl PeerAdmissionRepository for Repository<'_> {
	type Scope = Owned;
	fn node_id(&self) -> &str {
		&self.runtime.config.node_id
	}
	async fn request(&self, source: &str, path: &str, input: &Value) -> Result<Value> {
		crate::authorization::peer::authority_request(self.runtime, source, path, input)
			.await
			.map_err(Into::into)
	}
	async fn mapped(&self, source: &str, tenant: &str, subject: &str) -> Result<Owned> {
		let access =
			crate::authorization::peer::access(self.runtime, source, tenant, subject).await?;
		Ok(Scope {
			runtime: self.runtime.clone(),
			access: Box::new(access),
		})
	}
	async fn verify_record(&self, id: Uuid, source: &str) -> Result<Option<State>> {
		let f = self.runtime;
		let connection = f.store.orm_connection()?;
		crate::apps::federation::peer::models::AuthorizationRemoteAdmission::by_id(
			&mut connection.handle(),
			id,
			source,
		)
		.await
		.map(|row| row.map(Into::into))
		.map_err(Into::into)
	}
	async fn status_record(&self, source: &str, grant: Uuid) -> Result<Option<State>> {
		let result: NativeResult<Option<State>> = async {
			let f = self.runtime;
			let input =
				crate::apps::identity::serializers::peer_admission::Input { grant_id: grant };
			let row: Option<Record> = {
				let query_bind_1 = source;
				let query_bind_2 = input.grant_id;
				crate::database::native::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("authorization_remote_admissions"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(source_node=? AND grant_id=?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&f.store.pool)
				.await?
			};
			Ok(row.map(Into::into))
		}
		.await;
		result.map_err(Into::into)
	}
	async fn status_run(&self, id: Uuid, source: &str) -> Result<Option<RunInspection>> {
		let result: NativeResult<Option<RunInspection>> = async {
			let f = self.runtime;
			let record_id = id;
			let raw: Option<crate::domain::run_state::RawRun> = {
				let query_bind_1 = record_id;
				let query_bind_2 = source;
				aidash_server::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("runs"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=? AND home_node=?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&f.store.pool)
				.await?
			};
			Ok(raw.map(crate::domain::run_state::RawRun::inspect))
		}
		.await;
		result.map_err(Into::into)
	}
	async fn leaf_record(&self, source: &str, grant: Uuid, id: Uuid) -> Result<Option<State>> {
		let result: NativeResult<Option<State>> = async {
			let f = self.runtime;
			let admission = id;
			let row: Option<Record> = {
				let query_bind_1 = admission;
				let query_bind_2 = source;
				let query_bind_3 = grant;
				crate::database::native::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("authorization_remote_admissions"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=? AND source_node=? AND grant_id=?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
								Expr::value(query_bind_3.to_owned()).into(),
							],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&f.store.pool)
				.await?
			};
			Ok(row.map(Into::into))
		}
		.await;
		result.map_err(Into::into)
	}
	async fn run(&self, id: Uuid) -> Result<Run> {
		self.runtime.store.run(id).await.map_err(Into::into)
	}
	async fn inspect_run(&self, id: Uuid) -> Result<RunInspection> {
		self.runtime.store.inspect_run(id).await.map_err(Into::into)
	}
	async fn control(&self, id: Uuid, action: &RemoteExecutionControl) -> Result<RunInspection> {
		self.runtime
			.store
			.control(id, action.action())
			.await
			.map_err(Into::into)
	}
	fn messages(&self, run: &Run) -> Box<dyn AdmissionMessages + '_> {
		Box::new(crate::federation::Home::new(
			self.runtime.clone(),
			run.clone(),
		))
	}
	async fn message_limit(&self, run: &Run) -> Result<usize> {
		self.runtime
			.run_message_limit(run)
			.await
			.map_err(Into::into)
	}
	async fn message_recorded(&self, id: Uuid, key: &str, content: &str) -> bool {
		self.runtime
			.store
			.run_input_sequence(id, key, content)
			.await
			.is_ok()
	}
	async fn deliver(&self, run: &Run) -> Result<()> {
		self.runtime
			.deliver_run_messages(run)
			.await
			.map_err(Into::into)
	}
	fn notify(&self) {
		self.runtime.notify.notify_waiters()
	}
}
#[async_trait]
impl AdmissionMessages for crate::federation::Home {
	async fn reserve(&self, key: &str, content: &str) -> Result<bool> {
		self.reserve_run_message(key, content)
			.await
			.map_err(Into::into)
	}
	async fn release(&self, key: &str) -> Result<()> {
		self.release_run_messages(&[key.to_owned()])
			.await
			.map_err(Into::into)
	}
	async fn commit(&self, key: &str, content: &str) -> Result<()> {
		self.commit_run_message(key, content)
			.await
			.map_err(Into::into)
	}
}
impl From<aidash_domain::federation::execution::admission::Admission>
	for crate::apps::identity::serializers::peer_admission::Admission
{
	fn from(v: aidash_domain::federation::execution::admission::Admission) -> Self {
		Self {
			id: v.id,
			source_node: v.source_node,
			grant_id: v.grant_id,
			task_id: v.task_id,
			expires_at: v.expires_at,
		}
	}
}
