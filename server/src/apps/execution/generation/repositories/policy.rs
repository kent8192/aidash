//! Native policy queries and owned authorization transactions.
use crate::{
	Error, Result,
	authorization::{access::Access, identity::Actor},
	store::Store,
};
use aidash_application::ports::generation::policy::{GenerationPolicies, PolicySession};
use aidash_domain::{
	generation::policy::{Policy, Spec},
	identity::Principal,
	registry::EntityRef,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, JoinType, LockType, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr, TableRef,
};
use serde_json::{Value, json};

pub(crate) struct NativePolicies {
	pub store: Store,
	pub actor: Actor,
	pub principal: Principal,
}
enum Mode {
	Operator(crate::database::native::Transaction),
	Subject(Box<Access>),
}
struct Session {
	node: String,
	mode: Mode,
}
impl Session {
	fn tx(&mut self) -> &mut crate::database::native::Transaction {
		match &mut self.mode {
			Mode::Operator(tx) => tx,
			Mode::Subject(access) => &mut access.tx,
		}
	}
	async fn finish<T: Send>(
		self,
		result: aidash_application::Result<T>,
	) -> aidash_application::Result<T> {
		match self.mode {
			Mode::Operator(tx) => match result {
				Ok(value) => {
					tx.commit().await?;
					Ok(value)
				}
				Err(error) => Err(error),
			},
			Mode::Subject(access) => access
				.finish(result.map_err(Error::from))
				.await
				.map_err(Into::into),
		}
	}
}
#[async_trait]
impl GenerationPolicies for NativePolicies {
	fn principal(&self) -> &Principal {
		&self.principal
	}
	async fn ids(&self, tenant: &str) -> aidash_application::Result<Vec<String>> {
		let ids: Vec<String> = {
			let query_bind_1 = &tenant;
			crate::database::native::query_scalar(
				&Query::select()
					.expr(SimpleExpr::from(Expr::col(Alias::new("id"))))
					.from(Alias::new("generation_policies"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(tenant = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.order_by_expr(SimpleExpr::from(Expr::col(Alias::new("id"))), Order::Asc)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_all(&self.store.pool)
			.await?
		};

		Ok(ids)
	}
	async fn begin(
		&self,
		_: &str,
		exclusive: bool,
	) -> aidash_application::Result<Box<dyn PolicySession>> {
		let mode = match &self.actor {
			Actor::Operator => {
				Mode::Operator(crate::database::native::begin(&self.store.pool).await?)
			}
			Actor::Subject(identity) => Mode::Subject(Box::new(if exclusive {
				Access::begin_exclusive(&self.store, identity).await
			} else {
				Access::begin(&self.store, identity).await
			}?)),
		};
		Ok(Box::new(Session {
			mode,
			node: self.store.node_id.clone(),
		}))
	}
}
#[async_trait]
impl PolicySession for Session {
	async fn bindings(
		&mut self,
		entry: &aidash_domain::registry::Entry,
	) -> aidash_application::Result<aidash_domain::registry::bindings::BindingSnapshot> {
		let node = self.node.clone();
		crate::apps::registry::repositories::bindings::preview(&mut **self.tx(), &node, entry).await
	}
	async fn decide(&mut self, id: &str, action: &str) -> aidash_application::Result<bool> {
		match &mut self.mode {
			Mode::Operator(_) => Ok(true),
			Mode::Subject(access) => access
				.decide(&crate::generation::resource(access, id), action)
				.await
				.map_err(Into::into),
		}
	}
	async fn bundle(&mut self, tenant: &str) -> aidash_application::Result<Value> {
		let tx = self.tx();
		let document: Value = {
			let query_bind_1 = tenant;
			crate::database::native::query_scalar(
				&Query::select()
					.expr(SimpleExpr::from(Expr::col(Alias::new("document"))))
					.from(Alias::new("authorization_bundles"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(tenant = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **tx)
			.await?
		}
		.ok_or_else(|| Error::NotFound("authorization policy".into()))?;

		Ok(document)
	}
	async fn previous(
		&mut self,
		tenant: &str,
		id: &str,
		expected: i64,
	) -> aidash_application::Result<Option<Value>> {
		let tx = self.tx();
		let previous: Option<Value> = {
			let query_bind_1 = tenant;
			let query_bind_2 = id;
			let query_bind_3 = expected;
			crate::database::native::query_scalar(
				&Query::select()
					.expr(SimpleExpr::from(Expr::col(Alias::new("spec"))))
					.from(Alias::new("generation_policies"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(tenant = ? AND id = ? AND revision = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **tx)
			.await?
		};

		Ok(previous)
	}
	async fn approved(
		&mut self,
		tenant: &str,
		reference: &EntityRef,
	) -> aidash_application::Result<Option<Value>> {
		if aidash_application::registry::system::builtin_reference(reference) {
			let node = self.node.clone();
			let entry = crate::apps::registry::repositories::bindings::system_definition(
				&mut **self.tx(),
				&node,
				reference,
			)
			.await?;
			return Ok(Some(serde_json::to_value(entry)?));
		}
		let tx = self.tx();
		let metadata: Option<Value> = {
			let query_bind_1 = tenant;
			let query_bind_2 = &reference.id;
			let query_bind_3 = &reference.version;
			crate::database::native::query_scalar(
				&Query::select()
					.expr(SimpleExpr::from(Expr::col((
						Alias::new("r"),
						Alias::new("metadata"),
					))))
					.from_as(Alias::new("authorization_catalog"), Alias::new("c"))
					.join(
						JoinType::InnerJoin,
						TableRef::table_alias(Alias::new("registry"), Alias::new("r")),
						Expr::cust("r.id = c.entry_id AND r.version = c.entry_version"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(c.tenant = ? AND c.entry_id = ? AND c.entry_version = ? AND c.enabled)"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
						],
					))
					.lock(LockType::Share)
					.lock_tables([Alias::new("c")])
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **tx)
			.await?
		};

		Ok(metadata)
	}
	async fn compare_and_set(
		&mut self,
		tenant: &str,
		id: &str,
		expected: i64,
		spec: &Spec,
	) -> aidash_application::Result<Option<i64>> {
		let tx = self.tx();
		let revision: Option<i64> = if expected == 0 {
			{
				let query_bind_1 = tenant;
				let query_bind_2 = id;
				let query_bind_3 = json!(spec);
				crate::database::native::query_scalar(
					&Query::insert()
						.into_table(Alias::new("generation_policies"))
						.columns([
							Alias::new("tenant"),
							Alias::new("id"),
							Alias::new("revision"),
							Alias::new("spec"),
						])
						.from_subquery(
							Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								))
								.expr(Expr::cust("1"))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								))
								.to_owned(),
						)
						.on_conflict(
							OnConflict::columns(["tenant", "id"])
								.do_nothing()
								.to_owned(),
						)
						.returning_exprs([SimpleExpr::from(Expr::col(Alias::new("revision")))])
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **tx)
				.await?
			}
		} else {
			{
				let query_bind_1 = tenant;
				let query_bind_2 = id;
				let query_bind_3 = expected;
				let query_bind_4 = json!(spec);
				crate::database::native::query_scalar(
					&Query::update()
						.table(Alias::new("generation_policies"))
						.value_expr(Alias::new("revision"), Expr::cust("revision + 1"))
						.value_expr(
							Alias::new("spec"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"(tenant = ? AND id = ? AND revision = ?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
								Expr::value(query_bind_3.to_owned()).into(),
							],
						))
						.returning_exprs([SimpleExpr::from(Expr::col(Alias::new("revision")))])
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **tx)
				.await?
			}
		};

		Ok(revision)
	}
	async fn history(
		&mut self,
		tenant: &str,
		id: &str,
		revision: i64,
		spec: &Spec,
		actor: &str,
	) -> aidash_application::Result<()> {
		let tx = self.tx();

		{
			let query_bind_1 = tenant;
			let query_bind_2 = id;
			let query_bind_3 = revision;
			let query_bind_4 = json!(spec);
			let query_bind_5 = actor;
			crate::database::native::query(
				&Query::insert()
					.into_table(Alias::new("generation_policy_history"))
					.columns([
						Alias::new("tenant"),
						Alias::new("policy_id"),
						Alias::new("revision"),
						Alias::new("spec"),
						Alias::new("actor"),
					])
					.from_subquery(
						Query::select()
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
							.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await?
		};

		Ok(())
	}
	async fn load(
		&mut self,
		tenant: &str,
		id: &str,
		exclusive: bool,
	) -> aidash_application::Result<Policy> {
		load(self.tx(), tenant, id, exclusive)
			.await
			.map_err(Into::into)
	}
	async fn finish_update(
		self: Box<Self>,
		result: aidash_application::Result<Policy>,
	) -> aidash_application::Result<Policy> {
		(*self).finish(result).await
	}
	async fn finish_list(
		self: Box<Self>,
		result: aidash_application::Result<Vec<Policy>>,
	) -> aidash_application::Result<Vec<Policy>> {
		(*self).finish(result).await
	}
}

pub(crate) async fn load(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	id: &str,
	exclusive: bool,
) -> Result<Policy> {
	let query = if exclusive {
		Query::select()
			.expr(SimpleExpr::from(Expr::col(Alias::new("revision"))))
			.expr(SimpleExpr::from(Expr::col(Alias::new("spec"))))
			.expr(SimpleExpr::from(Expr::col(Alias::new("generated_count"))))
			.expr(SimpleExpr::from(Expr::col(Alias::new("allocated_tokens"))))
			.expr(SimpleExpr::from(Expr::col(Alias::new(
				"allocated_compaction_calls",
			))))
			.expr(SimpleExpr::from(Expr::col(Alias::new(
				"allocated_embedding_calls",
			))))
			.from(Alias::new("generation_policies"))
			.and_where(Expr::cust("tenant = $1 AND id = $2"))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder)
	} else {
		Query::select()
			.expr(SimpleExpr::from(Expr::col(Alias::new("revision"))))
			.expr(SimpleExpr::from(Expr::col(Alias::new("spec"))))
			.expr(SimpleExpr::from(Expr::col(Alias::new("generated_count"))))
			.expr(SimpleExpr::from(Expr::col(Alias::new("allocated_tokens"))))
			.expr(SimpleExpr::from(Expr::col(Alias::new(
				"allocated_compaction_calls",
			))))
			.expr(SimpleExpr::from(Expr::col(Alias::new(
				"allocated_embedding_calls",
			))))
			.from(Alias::new("generation_policies"))
			.and_where(Expr::cust("tenant = $1 AND id = $2"))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder)
	};
	let row: Option<(i64, Value, i64, i64, i64, i64)> = crate::database::native::query_as(&query)
		.columns(&[
			"revision",
			"spec",
			"generated_count",
			"allocated_tokens",
			"allocated_compaction_calls",
			"allocated_embedding_calls",
		])
		.bind(tenant)
		.bind(id)
		.fetch_optional(&mut **tx)
		.await?;
	let (
		revision,
		spec,
		generated_count,
		allocated_tokens,
		allocated_compaction_calls,
		allocated_embedding_calls,
	) = row.ok_or_else(|| Error::NotFound("generation policy".into()))?;
	Ok(Policy {
		tenant: tenant.into(),
		id: id.into(),
		revision,
		spec: serde_json::from_value(spec)?,
		generated_count,
		allocated_tokens,
		allocated_compaction_calls,
		allocated_embedding_calls,
	})
}
