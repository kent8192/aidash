use super::*;

#[rstest::rstest]
#[tokio::test]
async fn revoked_mounted_reference_is_deferred_and_reclaimed_after_detaching(
	#[future] capability_fixture: CoreFixture,
	#[from(worker_control)] control: WorkerControl,
) {
	let c = Box::pin(capability_fixture).await;
	let run = admit(&c).await;
	let (status, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let area: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let id = Uuid::new_v4();
	let file = Uuid::new_v4();
	let bytes = b"mounted reference";
	let digest = aidash_server::capabilities::objects::digest(bytes);
	let entry = json!({"file_id":file,"path":format!("{id}/original.txt"),"digest":digest,"size":bytes.len(),"media_type":"text/plain","scope":"references","provenance":{"kind":"reference","id":id}});
	let pool = c.f.store.pool.fixture_driver();
	fixture_blob(pool, &c.root, &digest, bytes).await;
	sqlx::query("INSERT INTO core_quotas (tenant, used_bytes) VALUES ('acme', $1) ON CONFLICT (tenant) DO UPDATE SET used_bytes=core_quotas.used_bytes+$1")
        .bind(bytes.len() as i64).execute(pool).await.unwrap();
	sqlx::query("INSERT INTO core_objects (id,tenant,kind,digest,size) VALUES ($1,'acme','reference_original',$2,$3)")
        .bind(file).bind(&digest).bind(bytes.len() as i64).execute(pool).await.unwrap();
	sqlx::query("INSERT INTO core_records (id,tenant,owner,kind,state,revision,data,expires_at) VALUES ($1,'acme','alice','reference','revoked',1,$2,CURRENT_TIMESTAMP)")
        .bind(id).bind(json!({"original":entry,"extraction":null,"chunks":[],"runner_released":false,"objects_released":false,"operation_id":null,"instance":null}))
        .execute(pool).await.unwrap();
	fixture_revision(pool, area, 1, json!([entry])).await;
	let worker = tokio::spawn(aidash_server::capabilities::reclamation::run(
		c.f.store.clone(),
		control.receiver,
	));
	let deferred = tokio::time::timeout(std::time::Duration::from_secs(12), async {
		loop {
			let (revision, data): (i64, Value) =
				sqlx::query_as("SELECT revision,data FROM core_records WHERE id=$1")
					.bind(id)
					.fetch_one(pool)
					.await
					.unwrap();
			if revision > 1 {
				assert_eq!(data["objects_released"], false);
				assert_eq!(data["original"]["file_id"], json!(file));
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(100)).await;
		}
	})
	.await
	.is_ok();
	fixture_revision(pool, area, 2, json!([])).await;
	let reclaimed = tokio::time::timeout(std::time::Duration::from_secs(12), async {
		loop {
			let object: Option<Uuid> =
				sqlx::query_scalar("SELECT id FROM core_objects WHERE id=$1")
					.bind(file)
					.fetch_optional(pool)
					.await
					.unwrap();
			if object.is_none() {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(100)).await;
		}
	})
	.await
	.is_ok();
	control.stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
	assert!(
		deferred,
		"mounted Reference revocation repeatedly rolled back"
	);
	assert!(reclaimed, "detached Reference object was not reclaimed");
}
