use super::*;

pub(super) async fn insert_run(f: &Federation, control: &str) -> Uuid {
	let id = Uuid::new_v4();
	sqlx::query(
		&Query::insert()
			.into_table(a("runs"))
			.columns(
				[
					"id",
					"task_id",
					"workspace_id",
					"home_node",
					"agent_id",
					"agent_version",
					"control",
				]
				.map(a),
			)
			.values_panic::<_, reinhardt::query::Value>([
				id.into(),
				Uuid::new_v4().into(),
				Uuid::new_v4().into(),
				f.config.node_id.clone().into(),
				"fixture".into(),
				"1.0.0".into(),
				control.into(),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.pool.driver())
	.await
	.unwrap();
	id
}

#[rstest::rstest]
#[tokio::test]
async fn ordering_release_only_notifies_the_next_unblocked_run(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let runtime = runtime.await;
	let (f, url, schema) = runtime.parts();
	let area = Uuid::new_v4();
	sqlx::query(
		&Query::insert()
			.into_table(a("core_areas"))
			.columns(
				[
					"id",
					"tenant",
					"home_node",
					"agent_id",
					"owner",
					"workspace_id",
					"thread_id",
				]
				.map(a),
			)
			.values_panic::<_, reinhardt::query::Value>([
				area.into(),
				"default".into(),
				f.config.node_id.clone().into(),
				"fixture".into(),
				"fixture".into(),
				Uuid::new_v4().into(),
				Uuid::new_v4().into(),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.pool.driver())
	.await
	.unwrap();
	let mut runs = Vec::new();
	for sequence in 0..5_i64 {
		// A paused head still owns its position; successors must not skip it.
		let run = insert_run(&f, if sequence == 3 { "PAUSED" } else { "ACTIVE" }).await;
		sqlx::query(
			&Query::insert()
				.into_table(a("core_runs"))
				.columns(["run_id", "area_id", "sequence"].map(a))
				.values_panic::<_, reinhardt::query::Value>([
					run.into(),
					area.into(),
					sequence.into(),
				])
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
		runs.push(run);
	}
	let mut expected = Vec::new();
	// Cancelling a non-head cannot unblock anything. Later releases skip that
	// terminal gap and each append exactly one notification, including a paused head.
	for (index, phase, next) in [
		(2, "CANCELLED", None),
		(0, "COMPLETED", Some(1)),
		(1, "FAILED", Some(3)),
		(3, "CANCELLED", Some(4)),
		(4, "COMPLETED", None),
	] {
		sqlx::query(
			&Query::update()
				.table(a("runs"))
				.value(a("phase"), phase)
				.and_where(Expr::col(a("id")).eq(Expr::value(runs[index])))
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
		if let Some(next) = next {
			expected.push(runs[next]);
		}
		let actual: Vec<Uuid> = sqlx::query_scalar(
			&Query::select()
				.column(a("run_id"))
				.from(a("run_activations"))
				.and_where(Expr::col(a("reason")).eq("ordering_release"))
				.order_by(a("generation"), reinhardt::query::Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(f.store.pool.driver())
		.await
		.unwrap();
		assert_eq!(actual, expected, "unexpected release after run {index}");
	}
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn visibility_release_retries_existing_work_without_fanout(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let runtime = runtime.await;
	let (f, url, schema) = runtime.parts();
	let environment = runtime.environment();
	for _ in 0..32 {
		insert_run(&f, "PAUSED").await;
	}
	let obligations = count(&f, "TRUE").await;
	let transaction = Uuid::new_v4();
	sqlx::query(
		&Query::insert()
			.into_table(a("atomic_participants"))
			.columns(["id", "coordinator", "digest", "manifest", "phase"].map(a))
			.values_panic::<_, reinhardt::query::Value>([
				transaction.into(),
				f.config.node_id.clone().into(),
				"fixture".into(),
				json!({}).into(),
				"PREPARED".into(),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.control_pool.driver())
	.await
	.unwrap();
	sqlx::query(
		&Query::update()
			.table(a("atomic_gate"))
			.value(a("transaction_id"), transaction)
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.control_pool.driver())
	.await
	.unwrap();
	let settings = Settings {
		namespace: schema.clone(),
		..Default::default()
	};
	let broker = Broker::provision(&environment.nats_url, &f.config.node_id, &settings)
		.await
		.unwrap();
	let runtime = aidash_server::activation::Runtime::new(f.clone(), settings, false);
	let (stop, stopping) = tokio::sync::watch::channel(false);
	let task = tokio::spawn(runtime.run(stopping));
	tokio::time::sleep(Duration::from_secs(1)).await;
	assert_eq!(count(&f, "published_at IS NOT NULL").await, 0);
	sqlx::query(
		&Query::update()
			.table(a("atomic_gate"))
			.value(a("transaction_id"), Option::<Uuid>::None)
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.control_pool.driver())
	.await
	.unwrap();
	let after_release = count(&f, "TRUE").await;
	// Existing durable obligations resume publication without a replacement row.
	wait_count(&f, "published_at IS NOT NULL", obligations).await;
	stop.send_replace(true);
	task.await.unwrap().unwrap();
	broker
		.context
		.delete_stream(&broker.stream_name)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
	assert_eq!(
		after_release, obligations,
		"gate release activated unrelated paused Runs"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn approval_notifications_target_only_the_bound_run(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let runtime = runtime.await;
	let (f, url, schema) = runtime.parts();
	let target = insert_run(&f, "ACTIVE").await;
	let unrelated = insert_run(&f, "ACTIVE").await;
	let approval = Uuid::new_v4();
	// A stale/mismatched pointer on another Run must not receive this approval.
	for run in [target, unrelated] {
		sqlx::query(
			&Query::update()
				.table(a("runs"))
				.value(a("phase"), "WAITING")
				.value(
					a("pending"),
					common::pending(aidash_server::domain::RunState::Waiting(Box::new(
						aidash_server::domain::WaitingState::CoreApproval {
							approval_id: approval,
							resume: Default::default(),
						},
					))),
				)
				.and_where(Expr::col(a("id")).eq(reinhardt::query::Expr::value(run)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	}
	for (id, kind, data) in [
		(Uuid::new_v4(), "file", json!({"run_id":"not-a-run"})),
		(Uuid::new_v4(), "grant", json!({"run_id":target})),
		(approval, "approval", json!({"run_id":target})),
	] {
		sqlx::query(
			&Query::insert()
				.into_table(a("core_records"))
				.columns(["id", "tenant", "owner", "kind", "state", "data"].map(a))
				.values_panic::<_, reinhardt::query::Value>([
					id.into(),
					"default".into(),
					"fixture".into(),
					kind.into(),
					"pending".into(),
					data.into(),
				])
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	}
	sqlx::query(
		&Query::update()
			.table(a("core_records"))
			.value(a("state"), "allowed")
			.and_where(Expr::col(a("id")).eq(reinhardt::query::Expr::value(approval)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.pool.driver())
	.await
	.unwrap();
	let actual: Vec<Uuid> = sqlx::query_scalar(
		&Query::select()
			.column(a("run_id"))
			.from(a("run_activations"))
			.and_where(Expr::col(a("reason")).eq("approval"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(f.store.pool.driver())
	.await
	.unwrap();
	cleanup(f, &url, &schema).await;
	assert_eq!(actual, vec![target, target]);
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
