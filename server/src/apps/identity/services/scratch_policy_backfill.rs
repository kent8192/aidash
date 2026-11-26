//! Upgrade the original member policy for named Scratch Space creation.

pub(crate) fn forward() -> String {
	use crate::apps::identity::services::duty_policy_backfill;
	use reinhardt::query::{Alias, Expr, ExprTrait, Query, SimpleExpr};
	// The member policy as it stood when this migration was written, so the
	// generated asset keeps the text it was applied with: every later grant and
	// rename has its own migration (`identity/0080_schedule_member_policy`
	// onwards).
	let mut current = duty_policy_backfill::member_policy_after();
	current
		.actions
		.remove(duty_policy_backfill::SCHEDULE_CREATE);
	let mut previous = current.clone();
	previous.actions.remove("scratch.create");
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
	let mut update = Query::update();
	update
		.table(Alias::new("authorization_bundles"))
		.value_expr(Alias::new("document"), policies)
		.value_expr(Alias::new("revision"), Expr::col("revision").add(1))
		.value_expr(Alias::new("updated_at"), Expr::cust("CURRENT_TIMESTAMP"))
		.and_where(matches_default);
	super::policy_revision_backfill::record_update(
		&mut update,
		"migration:identity/0067_scratch_member_policy",
	)
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
	async fn default_member_policy_is_upgraded_without_broadening_custom_policies(
		#[future] database: DatabaseFixture,
	) {
		let database = database.await;
		for tenant in ["scratch-default-policy", "scratch-custom-policy"] {
			let scope = database.tenant(tenant).await;
			let mut bundle =
				aidash_domain::identity::tenancy::initial_policy(tenant, false).unwrap();
			let policy = bundle
				.policies
				.iter_mut()
				.find(|policy| policy.id == "members")
				.unwrap();
			// The member policy of this migration's time, before its grant.
			*policy = crate::apps::identity::services::duty_policy_backfill::member_policy_after();
			policy
				.actions
				.remove(crate::apps::identity::services::duty_policy_backfill::SCHEDULE_CREATE);
			policy.actions.remove("scratch.create");
			if tenant == "scratch-custom-policy" {
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
		let mut tx = database.begin_fixture().await;
		tx.execute(&super::forward(), vec![]).await.unwrap();
		for (tenant, expected) in [
			("scratch-default-policy", true),
			("scratch-custom-policy", false),
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
			assert_eq!(policy.actions.contains("scratch.create"), expected);
			assert_eq!(policy.actions.contains("workspace.create"), expected);
		}
		tx.rollback().await.unwrap();
	}
}
