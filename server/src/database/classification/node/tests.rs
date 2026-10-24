use super::*;

#[test]
fn node_purge_requires_an_explicit_classification_for_added_tables() {
	// A migration must make the ownership decision, even for Deployment-wide
	// metadata. An unknown table therefore refuses the worker before erasure.
	assert!(ownership("unclassified_node_owned_records").is_err());
	for table in crate::database::classification::TABLES {
		assert!(ownership(table.name).is_ok(), "{}", table.name);
	}
}
