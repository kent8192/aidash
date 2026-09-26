use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};

#[rstest::fixture]
fn upload_fixture(
	#[future] capability_fixture: CoreFixture,
) -> impl std::future::Future<Output = (CoreFixture, String, Vec<u8>)> {
	let capability_fixture = Box::pin(capability_fixture);
	async move {
		let c = capability_fixture.await;
		let bytes = vec![b'x'; (4 << 20) + 3];
		let (status, upload) = request(&c.app, &c.token, "POST", "/api/references/uploads", json!({"idempotency_key":Uuid::new_v4(),"name":"bounded.txt","media_type":"text/plain","size":bytes.len(),"digest":aidash::capabilities::objects::digest(&bytes)})).await;
		assert_eq!(status, 200, "{upload}");
		let path = format!(
			"/api/references/{}",
			upload["reference_id"].as_str().unwrap()
		);
		(c, path, bytes)
	}
}

#[rstest::rstest]
#[tokio::test]
async fn tiny_upload_chunks_allocate_nothing_and_full_chunks_remain_idempotent(
	#[future] upload_fixture: (CoreFixture, String, Vec<u8>),
) {
	let (c, path, bytes) = Box::pin(upload_fixture).await;
	let chunk_path = format!("{path}/chunks");
	let (status, rejected) = request(
		&c.app,
		&c.token,
		"POST",
		&chunk_path,
		json!({"offset":0,"data":STANDARD.encode(&bytes[..1])}),
	)
	.await;
	assert_eq!(status, 400, "{rejected}");
	let count: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::col(Alias::new("id")).count())
			.from(Alias::new("core_objects"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&c.f.store.pool)
	.await
	.unwrap();
	assert_eq!(count, 0);
	let input = json!({"offset":0,"data":STANDARD.encode(&bytes[..4<<20])});
	let (status, uploaded) = request(&c.app, &c.token, "POST", &chunk_path, input.clone()).await;
	assert_eq!(status, 200, "{uploaded}");
	assert_eq!(uploaded["uploaded_bytes"], 4 << 20);
	assert_eq!(
		request(&c.app, &c.token, "POST", &chunk_path, input).await,
		(200, uploaded)
	);
	assert_eq!(
		request(
			&c.app,
			&c.token,
			"POST",
			&chunk_path,
			json!({"offset":4<<20,"data":STANDARD.encode(&bytes[..1])})
		)
		.await
		.0,
		400
	);
	let (status, uploaded) = request(
		&c.app,
		&c.token,
		"POST",
		&chunk_path,
		json!({"offset":4<<20,"data":STANDARD.encode(&bytes[4<<20..])}),
	)
	.await;
	assert_eq!(status, 200, "{uploaded}");
	assert_eq!(uploaded["uploaded_bytes"], bytes.len());
	let count: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::col(Alias::new("id")).count())
			.from(Alias::new("core_objects"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&c.f.store.pool)
	.await
	.unwrap();
	assert_eq!(count, 2);
	c.close().await;
}

#[rstest::rstest]
#[case(".")]
#[case("..")]
#[tokio::test]
async fn reference_names_cannot_create_dot_segment_mounts(
	#[future] capability_fixture: CoreFixture,
	#[case] name: &str,
) {
	let c = Box::pin(capability_fixture).await;
	let (status, rejected) = request(&c.app, &c.token, "POST", "/api/references/uploads", json!({"idempotency_key":Uuid::new_v4(),"name":name,"media_type":"text/plain","size":1,"digest":aidash::capabilities::objects::digest(b"x")})).await;
	assert_eq!(status, 400, "{rejected}");
	assert_eq!(rejected["error"]["code"], "REFERENCE_UPLOAD_LIMIT");
	c.close().await;
}
