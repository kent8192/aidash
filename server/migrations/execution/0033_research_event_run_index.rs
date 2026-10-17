// reinhardt-migration-source: 1
// Evidence Revocation and the retention sweep redact a Run's research tool
// events (ADR 0046). A partial expression index on the event's Run serves
// those updates without scanning the whole event log.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0033_research_event_run_index", "execution")
		.database_only(true)
		.add_dependency("execution", "0032_research_originals_state")
		.add_operation(Operation::CreateNamedIndex {
			table: "events".to_owned(),
			name: "events_tool_run".to_owned(),
			columns: vec![],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some(
				r#"(kind = ANY (ARRAY['tool.started'::text, 'tool.completed'::text]))"#.to_owned(),
			),
			concurrently: false,
			expressions: Some(vec![r#"((data ->> 'run_id'::text))"#.to_owned()]),
			mysql_options: None,
			operator_class: None,
		})
}
