use super::*;

#[test]
fn an_added_table_cannot_silently_survive_or_join_node_purge() {
	// A migration must make the ownership decision, even for Deployment-wide
	// metadata. An unknown table therefore refuses the worker before erasure.
	assert!(ownership("unclassified_node_owned_records").is_err());
	for table in crate::database::classification::TABLES {
		assert!(ownership(table.name).is_ok(), "{}", table.name);
	}
}
