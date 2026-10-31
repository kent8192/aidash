//! Catalog optimistic updates and history retain the original query trees and transaction.
use crate::database::native::Pool;
use crate::{
	Result,
	authorization::Authorization,
	registry::{EntityRef, Entry},
};
use aidash_application::ports::catalog::{CatalogAdministrator, CatalogMutation};
use aidash_application::tenancy::TenancyScope;
use aidash_domain::identity::{
	Principal,
	catalog::{Binding, CatalogRestriction, RestrictionChange},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};

struct BindingRow {
	tenant: String,
	node: String,
	entry_id: String,
	entry_version: String,
	enabled: bool,
	revision: i64,
	restriction: Option<serde_json::Value>,
}
crate::native_record!(BindingRow {
	tenant,
	node,
	entry_id,
	entry_version,
	enabled,
	revision,
	restriction
});

impl From<BindingRow> for Binding {
	fn from(row: BindingRow) -> Self {
		Self {
			tenant: row.tenant,
			node: aidash_domain::nodes::NodeName::parse(&row.node).expect("stored Node name"),
			entry_id: row.entry_id,
			entry_version: row.entry_version,
			enabled: row.enabled,
			revision: row.revision,
			restriction: row.restriction.map(|value| {
				serde_json::from_value(value).expect("stored Catalog Approval restriction")
			}),
		}
	}
}

/// Render a restriction as `jsonb`, with absence as SQL NULL so an approval
/// without one keeps its old meaning: everyone the Tenant's policy allows.
fn restriction_value(restriction: Option<&CatalogRestriction>) -> Result<SimpleExpr> {
	match restriction {
		Some(restriction) => Ok(Expr::value(serde_json::to_value(restriction)?).cast_as("jsonb")),
		None => Ok(Expr::cust("NULL::jsonb").into()),
	}
}
pub(crate) struct NativeMutation<'a>(pub(crate) &'a mut crate::database::native::Transaction);
async fn registered(
	tx: &mut crate::database::native::Transaction,
	entry: &EntityRef,
) -> Result<bool> {
	let exists: bool = {
		let query_bind_1 = &entry.id;
		let query_bind_2 = &entry.version;
		crate::database::native::query_scalar(
			&Query::select()
				.expr(Expr::exists(
					Query::select()
						.expr(Expr::cust("1"))
						.from(Alias::new("registry"))
						.and_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"tenant",
									)))
									.eq(Expr::cust("current_setting('aidash.tenant', true)")),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id")))
										.eq(SimpleExpr::CustomWithExpr(
											"(?)".to_owned(),
											vec![Expr::value(query_bind_1.to_owned()).into()],
										)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"version",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_owned(),
				))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **tx)
		.await?
	};

	Ok(exists)
}
async fn compare_and_set(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	node: &aidash_domain::nodes::NodeName,
	entry: &EntityRef,
	expected_revision: i64,
	enabled: bool,
	restriction: RestrictionChange,
) -> Result<Option<Binding>> {
	// `None` keeps the stored restriction; `Some` replaces it, with `None` or an
	// empty restriction clearing it.
	let replacement = match restriction {
		RestrictionChange::Preserve => None,
		RestrictionChange::Replace(restriction) => Some(restriction),
	};
	let binding: Option<BindingRow> = if expected_revision == 0 {
		{
			let query_bind_1 = tenant;
			let query_bind_2 = &entry.id;
			let query_bind_3 = &entry.version;
			let query_bind_4 = enabled;
			let restriction_expr =
				restriction_value(replacement.as_ref().and_then(Option::as_ref))?;
			crate::database::native::query_as(
				&Query::insert()
					.into_table(Alias::new("authorization_catalog"))
					.columns([
						Alias::new("tenant"),
						Alias::new("node"),
						Alias::new("entry_id"),
						Alias::new("entry_version"),
						Alias::new("enabled"),
						Alias::new("revision"),
						Alias::new("restriction"),
					])
					.from_subquery(
						Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(Expr::value(node.as_str()))
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
							.expr(Expr::cust("1"))
							.expr(restriction_expr)
							.to_owned(),
					)
					.on_conflict(
						OnConflict::columns(["tenant", "node", "entry_id", "entry_version"])
							.do_nothing()
							.to_owned(),
					)
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.columns(&[
				"tenant",
				"node",
				"entry_id",
				"entry_version",
				"enabled",
				"revision",
				"restriction",
			])
			.fetch_optional(&mut **tx)
			.await?
		}
	} else {
		{
			let query_bind_1 = tenant;
			let query_bind_2 = &entry.id;
			let query_bind_3 = &entry.version;
			let query_bind_4 = enabled;
			let query_bind_5 = expected_revision;
			let mut query = Query::update();
			query
				.table(Alias::new("authorization_catalog"))
				.value_expr(
					Alias::new("enabled"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_4.to_owned()).into()],
					),
				)
				.value_expr(Alias::new("revision"), Expr::cust("revision+1"));
			if let Some(restriction) = &replacement {
				query.value_expr(
					Alias::new("restriction"),
					restriction_value(restriction.as_ref())?,
				);
			}
			crate::database::native::query_as(
				&query
					.and_where(
						Condition::all()
							.add(
								reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant")))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_1.to_owned()).into()],
									)),
							)
							.add(
								reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("node")))
									.eq(Expr::value(node.as_str())),
							)
							.add(
								reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
									"entry_id",
								)))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								)),
							)
							.add(
								reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
									"entry_version",
								)))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								)),
							)
							.add(
								reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
									"revision",
								)))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_5.to_owned()).into()],
								)),
							),
					)
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **tx)
			.await?
		}
	};

	Ok(binding.map(Into::into))
}
async fn history(
	tx: &mut crate::database::native::Transaction,
	binding: &Binding,
	actor: &str,
	provider_warning_acknowledgment: Option<&str>,
) -> Result<()> {
	let tenant = binding.tenant.as_str();
	let entry = EntityRef {
		id: binding.entry_id.clone(),
		version: binding.entry_version.clone(),
	};
	let enabled = binding.enabled;

	{
		let query_bind_1 = tenant;
		let query_bind_2 = &entry.id;
		let query_bind_3 = &entry.version;
		let query_bind_4 = binding.revision;
		let query_bind_5 = enabled;
		let query_bind_6 = actor;
		let restriction = restriction_value(binding.restriction.as_ref())?;
		crate::database::native::query(
			&Query::insert()
				.into_table(Alias::new("authorization_catalog_history"))
				.columns([
					Alias::new("tenant"),
					Alias::new("node"),
					Alias::new("entry_id"),
					Alias::new("entry_version"),
					Alias::new("revision"),
					Alias::new("enabled"),
					Alias::new("restriction"),
					Alias::new("actor"),
					Alias::new("provider_warning_acknowledgment"),
				])
				.from_subquery(
					Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(Expr::value(binding.node.as_str()))
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
						.expr(restriction)
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_6.to_owned()).into()],
						))
						.expr(Expr::value(
							provider_warning_acknowledgment.map(str::to_owned),
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
pub(crate) async fn bindings(
	pool: &Pool,
	tenant: &str,
	node: &aidash_domain::nodes::NodeName,
) -> Result<Vec<Binding>> {
	let rows: Vec<BindingRow> = {
		let query_bind_1 = tenant;
		crate::database::native::query_as(
			&Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("authorization_catalog"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.and_where(Expr::col("node").eq(Expr::value(node.as_str())))
				.order_by(Alias::new("entry_id"), Order::Asc)
				.order_by(Alias::new("entry_version"), Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(pool.in_scope(&TenancyScope::tenant(tenant)?))
		.await?
	};
	Ok(rows.into_iter().map(Into::into).collect())
}

#[async_trait]
impl CatalogMutation for NativeMutation<'_> {
	async fn registered(&mut self, reference: &EntityRef) -> aidash_application::Result<bool> {
		registered(self.0, reference).await.map_err(Into::into)
	}
	async fn compare_and_set(
		&mut self,
		tenant: &str,
		node: &aidash_domain::nodes::NodeName,
		reference: &EntityRef,
		expected_revision: i64,
		enabled: bool,
		restriction: RestrictionChange,
	) -> aidash_application::Result<Option<Binding>> {
		compare_and_set(
			self.0,
			tenant,
			node,
			reference,
			expected_revision,
			enabled,
			restriction,
		)
		.await
		.map_err(Into::into)
	}
	async fn history(
		&mut self,
		binding: &Binding,
		actor: &str,
		provider_warning_acknowledgment: Option<&str>,
	) -> aidash_application::Result<()> {
		history(self.0, binding, actor, provider_warning_acknowledgment)
			.await
			.map_err(Into::into)
	}
}
pub(crate) struct NativeAdministrator<'a> {
	pub(crate) mutation: NativeMutation<'a>,
	pub(crate) principal: Principal,
}
#[async_trait]
impl CatalogMutation for NativeAdministrator<'_> {
	async fn registered(&mut self, reference: &EntityRef) -> aidash_application::Result<bool> {
		self.mutation.registered(reference).await
	}
	async fn compare_and_set(
		&mut self,
		tenant: &str,
		node: &aidash_domain::nodes::NodeName,
		reference: &EntityRef,
		expected_revision: i64,
		enabled: bool,
		restriction: RestrictionChange,
	) -> aidash_application::Result<Option<Binding>> {
		self.mutation
			.compare_and_set(
				tenant,
				node,
				reference,
				expected_revision,
				enabled,
				restriction,
			)
			.await
	}
	async fn history(
		&mut self,
		binding: &Binding,
		actor: &str,
		provider_warning_acknowledgment: Option<&str>,
	) -> aidash_application::Result<()> {
		self.mutation
			.history(binding, actor, provider_warning_acknowledgment)
			.await
	}
}
#[async_trait]
impl CatalogAdministrator for NativeAdministrator<'_> {
	fn principal(&self) -> &Principal {
		&self.principal
	}
	async fn load_tenant(&mut self, tenant: &str) -> aidash_application::Result<()> {
		Authorization::load(self.mutation.0, tenant)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn definition(&mut self, reference: &EntityRef) -> aidash_application::Result<Entry> {
		crate::apps::marketplace::repositories::definitions::raw(self.mutation.0, reference)
			.await
			.map_err(Into::into)
	}
}
