//! Stored policies after the Workspace was dissolved into Threads (#1008,
//! ADR 0140): the `workspace.*` actions become `thread.*`, the `workspace`
//! resource kind becomes `thread`, the `workspace_id` attribute a condition
//! reads (`/workspace_id`) becomes `thread_id`, and the trusted
//! `source_workspace_id` a receiving Node sets for a remote Run (#1007)
//! becomes `source_thread_id`, in every policy of every Tenant. A receiving
//! Node's policy also names a Home's Thread by its foreign resource id,
//! `{home}/workspaces/{id}`, which is now `{home}/threads/{id}`
//! (`authorization::tools::remote_identifier`); a deny that kept either old
//! name would stop matching and so fail open.
use reinhardt::query::{Alias, Expr, ExprTrait, Query, SimpleExpr};

/// The first five are actions (see `before_threads`); a condition names an
/// attribute by its JSON Pointer, so both the bare name and the pointer go.
/// Each is a whole JSON string, so `"workspace_id"` never touches
/// `"source_workspace_id"`.
const RENAMES: [(&str, &str); 10] = [
	("workspace.create", "thread.create"),
	("workspace.events", "thread.events"),
	("workspace.read", "thread.read"),
	("workspace.share", "thread.share"),
	("workspace.update", "thread.update"),
	("workspace_id", "thread_id"),
	("/workspace_id", "/thread_id"),
	("workspace", "thread"),
	("source_workspace_id", "source_thread_id"),
	("/source_workspace_id", "/source_thread_id"),
];

/// A JSON string that is a Home's foreign resource id of a Thread: a Node
/// address `aidash://<deployment>/<node>` followed by `/<collection>/`, the
/// only form a policy's `resources.ids` gives a remote Thread. Matching the
/// whole address keeps any other string that merely contains the collection
/// name, a URL for instance, as it is. The capture is the opening quote and
/// the address.
fn home_resource_id(collection: &str) -> String {
	format!("(\"aidash://[^\"/]+/[^\"/]+)/{collection}/")
}

fn rename(from_to: impl Fn((&str, &str)) -> (String, String)) -> String {
	let mut document = "document::text".to_owned();
	let mut values = Vec::new();
	for pair in RENAMES {
		let (from, to) = from_to(pair);
		document = format!("replace({document}, ?, ?)");
		values.push(Expr::value(format!("\"{from}\"")).into());
		values.push(Expr::value(format!("\"{to}\"")).into());
	}
	let (from, to) = from_to(("workspaces", "threads"));
	document = format!("regexp_replace({document}, ?, ?, 'g')");
	values.push(Expr::value(home_resource_id(&from)).into());
	values.push(Expr::value(format!("\\1/{to}/")).into());
	let mut matched = Vec::new();
	let mut any = Vec::new();
	for pair in RENAMES {
		let (from, _) = from_to(pair);
		matched.push("document::text LIKE ?".to_owned());
		any.push(Expr::value(format!("%\"{from}\"%")).into());
	}
	matched.push("document::text ~ ?".to_owned());
	any.push(Expr::value(home_resource_id(&from)).into());
	let mut update = Query::update();
	update
		.table(Alias::new("authorization_bundles"))
		.value_expr(
			Alias::new("document"),
			SimpleExpr::CustomWithExpr(format!("({document})::jsonb"), values),
		)
		.value_expr(Alias::new("revision"), Expr::col("revision").add(1))
		.value_expr(Alias::new("updated_at"), Expr::cust("CURRENT_TIMESTAMP"))
		.and_where(SimpleExpr::CustomWithExpr(
			format!("({})", matched.join(" OR ")),
			any,
		));
	super::policy_revision_backfill::record_update(
		&mut update,
		"migration:identity/0111_thread_member_policy",
	)
}

pub(crate) fn forward() -> String {
	format!(
		"SET LOCAL aidash.scope = 'deployment';\n{};\n",
		rename(|(from, to)| (from.to_owned(), to.to_owned()))
	)
}

/// Reversal names the Workspace actions, kind, attributes and ids again.
pub(crate) fn backward() -> String {
	format!(
		"SET LOCAL aidash.scope = 'deployment';\n{};\n",
		rename(|(from, to)| (to.to_owned(), from.to_owned()))
	)
}

/// The default member policy as it was stored before #1008, for the backfills
/// of earlier migrations whose text must not change: they compare stored
/// policies with the member policy of their time.
pub(crate) fn before_threads(
	mut policy: aidash_domain::policy::Policy,
) -> aidash_domain::policy::Policy {
	for (from, to) in RENAMES.iter().take(5) {
		if policy.actions.remove(*to) {
			policy.actions.insert((*from).to_owned());
		}
	}
	policy
}

#[cfg(test)]
mod tests {
	use crate::apps::execution::test_database::{DatabaseFixture, database};
	use reinhardt::db::orm::execution::convert_values;
	use reinhardt::query::{
		Alias, Expr, ExprTrait, IntoValue, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	use rstest::rstest;
	use serde_json::{Value, json};

	const HOME: &str = "aidash://calm-heron/able-oak-abcd1234";
	const THREAD: &str = "6f1c0b8e-4d0e-4c39-9a51-0d6c1a3b2e77";
	const ELSEWHERE: &str = "https://api.example.com/workspaces/42";

	/// A receiving Node's policy from before #1008: a deny on a Home Thread's
	/// trusted attribute, a deny on the Home Thread by its foreign id, and an
	/// id that only contains the old collection name.
	fn receiver_policy(tenant: &str, collection: &str, attribute: &str) -> Value {
		json!({
			"tenant": tenant,
			"subjects": {"alice": {"kind": "user"}},
			"policies": [
				{
					"id": "no-tools-for-that-thread",
					"effect": "deny",
					"subjects": {"any": true},
					"actions": ["tool.invoke"],
					"resources": {"kinds": ["tool"]},
					"condition": {
						"op": "eq",
						"left": {"source": "resource", "path": format!("/{attribute}")},
						"right": {"source": "literal", "value": THREAD}
					}
				},
				{
					"id": "not-that-thread",
					"effect": "deny",
					"subjects": {"any": true},
					"actions": ["task.delegate"],
					"resources": {
						"kinds": ["task"],
						"ids": [
							format!("{HOME}/{collection}/{THREAD}"),
							format!("{HOME}/tasks/{THREAD}"),
							ELSEWHERE
						]
					}
				}
			]
		})
	}

	/// The asset's statement after the `SET LOCAL` that opens Deployment Scope.
	fn update(asset: &str) -> String {
		let (scope, update) = asset.split_once('\n').unwrap();
		assert_eq!(scope, "SET LOCAL aidash.scope = 'deployment';");
		update.to_owned()
	}

	async fn stored(tx: &mut crate::database::native::Transaction, tenant: &str) -> (Value, i64) {
		let (sql, values) = Query::select()
			.columns(["document", "revision"].map(Alias::new))
			.from(Alias::new("authorization_bundles"))
			.and_where(Expr::col("tenant").eq(tenant))
			.build(PostgresQueryBuilder);
		let row = tx
			.fetch_optional(&sql, convert_values(values))
			.await
			.unwrap()
			.unwrap();
		let row = reinhardt::db::orm::QueryRow::from_backend_row(row);
		(row.get("document").unwrap(), row.get("revision").unwrap())
	}

	#[rstest]
	#[tokio::test]
	async fn a_receivers_denies_on_a_home_thread_keep_matching_after_the_rename(
		#[future] database: DatabaseFixture,
	) {
		// Arrange
		let database = database.await;
		let tenant = "thread-receiver-policy";
		let scope = database.tenant(tenant).await;
		let mut tx = database.begin_in(&scope).await;
		let (sql, values) = Query::insert()
			.into_table(Alias::new("authorization_bundles"))
			.columns(["tenant", "revision", "document"].map(Alias::new))
			.values_panic([
				tenant.into_value(),
				1_i64.into_value(),
				receiver_policy(tenant, "workspaces", "source_workspace_id").into_value(),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await.unwrap();
		tx.commit().await.unwrap();

		// Act: the migration's own Deployment Scope is the fixture's here.
		let mut tx = database.begin_fixture().await;
		tx.execute(&update(&super::forward()), vec![])
			.await
			.unwrap();

		// Assert: the attribute and the Home Thread's id name the Thread; the
		// Task id and an unrelated URL stay as they were.
		let (document, revision) = stored(&mut tx, tenant).await;
		assert_eq!(revision, 2);
		assert_eq!(
			document,
			receiver_policy(tenant, "threads", "source_thread_id")
		);
		serde_json::from_value::<aidash_domain::policy::PolicyBundle>(document).unwrap();

		// Reversal restores the Workspace names.
		tx.execute(&update(&super::backward()), vec![])
			.await
			.unwrap();
		let (document, revision) = stored(&mut tx, tenant).await;
		assert_eq!(revision, 3);
		assert_eq!(
			document,
			receiver_policy(tenant, "workspaces", "source_workspace_id")
		);
		tx.rollback().await.unwrap();
	}
}
