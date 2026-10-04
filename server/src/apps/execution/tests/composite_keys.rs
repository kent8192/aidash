use aidash_server::apps::identity::models::states::AuthorizationRunReadResourceKind as Kind;
use reinhardt::{
	db::orm::{DatabaseValue, Model, connection::OrmExecutor},
	model,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[path = "support/native_database.rs"]
mod native_database;
use native_database::{DatabaseFixture, database as database_fixture};

#[model(app_label = "repro", table_name = "composite_boundary_repro")]
#[derive(Serialize, Deserialize)]
pub struct Entry {
	#[field(primary_key = true, field_type = "uuid", db_column = "tenant_id")]
	pub tenant_key: Uuid,
	#[field(primary_key = true, field_type = "text", max_length = 64)]
	pub kind: Kind,
	#[field(primary_key = true, max_length = 64, db_column = "entry_id")]
	pub entry_key: String,
	#[field(max_length = 64)]
	pub body: String,
}

#[derive(Clone, Copy)]
enum Mutation {
	Fields,
	Model,
}

#[rstest::rstest]
#[case(Mutation::Fields)]
#[case(Mutation::Model)]
#[tokio::test]
async fn composite_updates_preserve_uuid_storage_and_neighboring_keys(
	#[case] mutation: Mutation,
	#[future] database_fixture: DatabaseFixture,
) {
	// Arrange: the three records share individual keys but not the complete identity.
	let fixture = database_fixture.await;
	let lease = &fixture.lease;
	let id = Uuid::new_v4();
	let other_tenant = Uuid::new_v4();
	let result: reinhardt::core::exception::Result<()> = lease.handle().atomic(async |tx| {
        OrmExecutor::execute(tx, "CREATE TEMP TABLE composite_boundary_repro (tenant_id uuid NOT NULL, kind text NOT NULL, entry_id text NOT NULL, body text NOT NULL, PRIMARY KEY (tenant_id, kind, entry_id)) ON COMMIT DROP", vec![]).await?;
        for (tenant, key, kind, body) in [(id, "a", Kind::Task, "old-a"), (id, "b", Kind::Task, "old-b"), (id, "a", Kind::Artifact, "old-artifact"), (other_tenant, "a", Kind::Task, "other-tenant")] {
            let row = Entry::build().tenant_key(tenant).kind(kind).entry_key(key).body(body).finish();
            assert_eq!(row.encode_database_fields().unwrap()["tenant_key"], DatabaseValue::Uuid(tenant));
            Entry::objects().create_with_conn(tx, &row).await?;
        }
        // Act: use every typed key, including aliased physical columns.
        match mutation {
            Mutation::Fields => {
                let changed = Entry::objects()
                .filter(Entry::field_tenant_key().eq(id))
                .filter(Entry::field_kind().eq(Kind::Task))
                .filter(Entry::field_entry_key().eq("a"))
                .update_fields_with_conn(tx, [(Entry::field_body(), "new-a")]).await?;
                assert_eq!(changed, 1);
            },
            Mutation::Model => {
                let row = Entry::build().tenant_key(id).kind(Kind::Task).entry_key("a").body("new-a").finish();
                Entry::objects().update_with_conn(tx, &row).await?;
            },
        };
        // Assert: only the complete matching identity may change.
        let rows = Entry::objects().all().all_with_db(tx).await?;
        assert_eq!(rows.len(), 4);
        for row in rows {
            if row.tenant_key == other_tenant {
                assert_eq!(row.kind, Kind::Task);
                assert_eq!(row.entry_key, "a");
                assert_eq!(row.body, "other-tenant");
                continue;
            }
            assert_eq!(row.tenant_key, id);
            let expected = match (&row.kind, row.entry_key.as_str()) {
                (Kind::Task, "a") => "new-a",
                (Kind::Task, "b") => "old-b",
                (Kind::Artifact, "a") => "old-artifact",
                _ => panic!("unexpected row"),
            };
            assert_eq!(row.body, expected);
        }
        Ok(())
    }).await;
	result.unwrap();
}
