// reinhardt-migration-source: 1
// Request Estimators (#179, ADR 0017; #367): admit the model `request_estimator`
// key the dashboard sends. Edit only the CURRENT registry_model_config read
// from the catalog, so the allowlist additions of 0016-0030 survive, and add
// registry_model_request_estimator to limit the value to `bytes` or
// `calibrated`. PostgreSQL JSONB CHECK expressions and catalog-driven
// constraint edits have no typed Reinhardt operation; the filesystem migration
// loader supports external assets through RunSQL only.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0031_model_request_estimator", "registry")
		.database_only(true)
		.add_dependency("registry", "0030_plan_connection_models")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0031_model_request_estimator.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0031_model_request_estimator.sql").into()),
		})
		.atomic(true)
}
