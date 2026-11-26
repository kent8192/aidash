//! Seed the same default Scratch Space as Node creation for fixtures that insert Nodes directly.

pub(super) async fn seed(
	tx: &mut sqlx::PgConnection,
	tenant: &str,
	node: &aidash_domain::nodes::NodeName,
) {
	let installed: bool = sqlx::query_scalar(
		"SELECT EXISTS (SELECT 1 FROM pg_attribute WHERE attrelid = to_regclass('scratch_spaces') AND attname = 'is_default' AND NOT attisdropped)",
	)
	.fetch_one(&mut *tx)
	.await
	.expect("the Scratch Space migration status is readable");
	if !installed {
		return;
	}
	let id: Option<uuid::Uuid> = sqlx::query_scalar(
		"INSERT INTO scratch_spaces (id, tenant, node, name, creator, state, is_default, created_at, last_used_at)
		 VALUES (gen_random_uuid(), $1, $2, 'Node Scratch Space', '', 'active', true, now(), now())
		 ON CONFLICT (tenant, node, is_default) WHERE is_default DO NOTHING
		 RETURNING id",
	)
	.bind(tenant)
	.bind(node.as_str())
	.fetch_optional(&mut *tx)
	.await
	.expect("the Node default Scratch Space is seeded");
	if let Some(id) = id {
		sqlx::query(
			"INSERT INTO scratch_space_settings (space_id, tenant, settings) VALUES ($1, $2, $3)",
		)
		.bind(id)
		.bind(serde_json::to_value(aidash_domain::scratch::ScratchSettings::for_node()).unwrap())
		.execute(tx)
		.await
		.expect("the Node default Scratch Space settings are seeded");
	}
}
