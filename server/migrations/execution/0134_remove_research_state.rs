// reinhardt-migration-source: 1
// ORM state of the removed Research tables; 0132 changed the database.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0134_remove_research_state", "execution")
		.add_dependency("execution", "0133_remove_research")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::DropTable {
			name: "research_page_images".into(),
		})
		.add_operation(Operation::DropTable {
			name: "research_originals".into(),
		})
		.add_operation(Operation::DropTable {
			name: "research_fragments".into(),
		})
		.add_operation(Operation::DropTable {
			name: "research_snapshots".into(),
		})
		.add_operation(Operation::DropTable {
			name: "research_sources".into(),
		})
		.add_operation(Operation::DropTable {
			name: "research_attempts".into(),
		})
		.add_operation(Operation::DropTable {
			name: "research_journal_results".into(),
		})
		.add_operation(Operation::DropTable {
			name: "research_revocations".into(),
		})
		.add_operation(Operation::DropTable {
			name: "research_account_terminations".into(),
		})
}
