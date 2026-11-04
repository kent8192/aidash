use super::*;
use reinhardt::query::{Alias, Expr, LockType, OnConflict, PostgresQueryBuilder, Query};

#[rstest::rstest]
#[tokio::test]
async fn locked_blob_does_not_stop_reclamation_of_other_blobs(
	#[future] capability_fixture: CoreFixture,
	#[from(worker_control)] control: WorkerControl,
) {
	let c = Box::pin(capability_fixture).await;
	let first = b"locked garbage";
	let second = b"other garbage";
	let first_digest = aidash_server::capabilities::objects::digest(first);
	let second_digest = aidash_server::capabilities::objects::digest(second);
	let pool = c.f.store.pool.fixture_driver();
	fixture_blob(pool, &c.root, &first_digest, first).await;
	fixture_blob(pool, &c.root, &second_digest, second).await;
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("core_quotas"))
			.columns([Alias::new("tenant")])
			.values(vec!["acme".into()])
			.unwrap()
			.on_conflict(
				OnConflict::column(Alias::new("tenant"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(pool)
	.await
	.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_quotas"))
			.value_expr(
				Alias::new("used_bytes"),
				Expr::col(Alias::new("used_bytes"))
					.add(Expr::value((first.len() + second.len()) as i64)),
			)
			.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
			.to_string(PostgresQueryBuilder),
	)
	.execute(pool)
	.await
	.unwrap();
	let mut owner = pool.begin().await.unwrap();
	sqlx::query(
		&Query::select()
			.column(Alias::new("digest"))
			.from(Alias::new("core_blobs"))
			.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
			.and_where(Expr::col(Alias::new("digest")).eq(first_digest.as_str()))
			.lock(LockType::KeyShare)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *owner)
	.await
	.unwrap();
	let worker = tokio::spawn(aidash_server::capabilities::reclamation::run(
		c.f.store.clone(),
		control.receiver,
	));
	let progressed = tokio::time::timeout(std::time::Duration::from_secs(12), async {
		loop {
			let exists: Option<String> = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("digest"))
					.from(Alias::new("core_blobs"))
					.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
					.and_where(Expr::col(Alias::new("digest")).eq(second_digest.as_str()))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(pool)
			.await
			.unwrap();
			if exists.is_none() {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(100)).await;
		}
	})
	.await
	.is_ok();
	owner.rollback().await.unwrap();
	control.stop.send(true).unwrap();
	tokio::time::timeout(std::time::Duration::from_secs(10), worker)
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	c.close().await;
	assert!(
		progressed,
		"a publisher-locked blob prevented other garbage collection"
	);
}
