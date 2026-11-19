//! Upgrade the default member policy so members may register Schedules (#557).

pub(crate) fn forward() -> String {
	use reinhardt::query::{
		Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
	};
	let bundle = aidash_domain::identity::tenancy::initial_policy("migration", false)
		.expect("the migration Tenant name is valid");
	let current = bundle
		.policies
		.into_iter()
		.find(|policy| policy.id == "members")
		.expect("the initial member policy exists");
	let mut previous = current.clone();
	previous.actions.remove(aidash_domain::schedules::CREATE);
	let previous = serde_json::to_value(previous).expect("the member policy serializes");
	let current = serde_json::to_value(current).expect("the member policy serializes");
	// Reinhardt Query has no builder for PostgreSQL's JSONB array transformation.
	// Keep custom syntax inside expressions while Query owns the UPDATE.
	let policies = SimpleExpr::CustomWithExpr(
		"jsonb_set(document, '{policies}', (SELECT jsonb_agg(CASE WHEN entry = ?::jsonb THEN ?::jsonb ELSE entry END ORDER BY ord) FROM jsonb_array_elements(document->'policies') WITH ORDINALITY AS p(entry, ord)))".into(),
		vec![Expr::value(previous.clone()).into(), Expr::value(current).into()],
	);
	let matches_default = SimpleExpr::CustomWithExpr(
		"document->'policies' @> jsonb_build_array(?::jsonb)".into(),
		vec![Expr::value(previous).into()],
	);
	Query::update()
		.table(Alias::new("authorization_bundles"))
		.value_expr(Alias::new("document"), policies)
		.value_expr(Alias::new("revision"), Expr::col("revision").add(1))
		.value_expr(Alias::new("updated_at"), Expr::cust("CURRENT_TIMESTAMP"))
		.and_where(matches_default)
		.to_string(PostgresQueryBuilder)
}

/// Reversal preserves policy edits made after this data migration.
pub(crate) fn backward() -> String {
	use reinhardt::query::{Expr, PostgresQueryBuilder, Query, QueryStatementBuilder};
	format!(
		"{};\n",
		Query::select()
			.expr(Expr::value(1))
			.to_string(PostgresQueryBuilder)
	)
}

#[cfg(test)]
mod tests {
	use crate::apps::execution::test_database::{DatabaseFixture, database};
	use reinhardt::db::orm::execution::convert_values;
	use reinhardt::query::{
		Alias, Expr, ExprTrait, IntoValue, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	use rstest::rstest;

	#[rstest]
	#[tokio::test]
	async fn default_member_policy_gains_schedule_create_and_custom_policies_do_not(
		#[future] database: DatabaseFixture,
	) {
		// Arrange: one Tenant keeps the default member policy as it was before
		// #557; another narrowed it.
		let database = database.await;
		for tenant in ["schedule-default-policy", "schedule-custom-policy"] {
			let scope = database.tenant(tenant).await;
			let mut bundle =
				aidash_domain::identity::tenancy::initial_policy(tenant, false).unwrap();
			let policy = bundle
				.policies
				.iter_mut()
				.find(|policy| policy.id == "members")
				.unwrap();
			policy.actions.remove(aidash_domain::schedules::CREATE);
			if tenant == "schedule-custom-policy" {
				policy.actions.remove("workspace.create");
			}
			let mut tx = database.begin_in(&scope).await;
			let (sql, values) = Query::insert()
				.into_table(Alias::new("authorization_bundles"))
				.columns(["tenant", "revision", "document"].map(Alias::new))
				.values_panic([
					tenant.into_value(),
					1_i64.into_value(),
					serde_json::to_value(bundle).unwrap().into_value(),
				])
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await.unwrap();
			tx.commit().await.unwrap();
		}
		// Act
		let mut tx = database.begin_fixture().await;
		tx.execute(&super::forward(), vec![]).await.unwrap();
		// Assert
		for (tenant, expected) in [
			("schedule-default-policy", true),
			("schedule-custom-policy", false),
		] {
			let (sql, values) = Query::select()
				.column(Alias::new("document"))
				.from(Alias::new("authorization_bundles"))
				.and_where(Expr::col("tenant").eq(tenant))
				.build(PostgresQueryBuilder);
			let row = tx
				.fetch_optional(&sql, convert_values(values))
				.await
				.unwrap()
				.unwrap();
			let row = reinhardt::db::orm::QueryRow::from_backend_row(row);
			let document: serde_json::Value = row.get("document").unwrap();
			let bundle: aidash_domain::policy::PolicyBundle =
				serde_json::from_value(document).unwrap();
			let policy = bundle
				.policies
				.iter()
				.find(|policy| policy.id == "members")
				.unwrap();
			assert_eq!(
				policy.actions.contains(aidash_domain::schedules::CREATE),
				expected
			);
			assert_eq!(policy.actions.contains("workspace.create"), expected);
		}
		tx.rollback().await.unwrap();
	}
}
