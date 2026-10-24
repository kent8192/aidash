// reinhardt-migration-source: 1
// Prompt-free evidence for every physical inference call, including Summary Stage.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0050_usage_evidence", "execution")
        .add_dependency("execution", "0049_foreign_key_relations_state")
        // Historical Usage Records were exclusively main inference. Populate them
        // once, then require every new dispatch to supply its purpose explicitly.
        .add_operation(Operation::AddColumn {
            table: "inference_usage".into(),
            column: ColumnDefinition::new("purpose", FieldType::Text).with_not_null(true).with_default(Some("'main'".into())),
            mysql_options: None,
        })
        .add_operation(Operation::AlterColumn {
            table: "inference_usage".into(), column: "purpose".into(),
            old_definition: Some(ColumnDefinition::new("purpose", FieldType::Text).with_not_null(true).with_default(Some("'main'".into()))),
            new_definition: ColumnDefinition::new("purpose", FieldType::Text).with_not_null(true),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "inference_usage".into(),
            column: ColumnDefinition::new("requested_route", FieldType::Text),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "inference_usage".into(),
            column: ColumnDefinition::new("provider_identity", FieldType::Jsonb),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "inference_usage".into(),
            column: ColumnDefinition::new("shadow_estimates", FieldType::Jsonb),
            mysql_options: None,
        })
        .atomic(true)
}
