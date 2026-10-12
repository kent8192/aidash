// reinhardt-migration-source: 1
// The Home records the Workspace corpus digest read before each remote
// retrieval, so a remote Ordered Run's reuse step is admitted only while the
// corpus is unchanged (#222). Existing operations keep NULL: they can no longer
// vouch for a reuse, and the Run retrieves once more.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0029_remote_operation_corpus_digest", "knowledge")
		.add_dependency("knowledge", "0028_provider_credentials")
		.add_operation(Operation::AddColumn {
			table: "semantic_remote_operations".into(),
			column: ColumnDefinition::new("corpus_digest", FieldType::Text),
			mysql_options: None,
		})
}
