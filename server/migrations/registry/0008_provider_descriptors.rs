// reinhardt-migration-source: 1
// PostgreSQL function bodies and NOT VALID checks are unsupported by typed
// migration operations. SQL assets contain DDL only, never legacy data rewrites.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0008_provider_descriptors", "registry")
        .database_only(true)
        .add_dependency("registry", "0007_model_state")
        .add_operation(Operation::DropConstraintDefinition { table:"registry".into(),constraint:Constraint::Check { name:"registry_kind_check".into(),expression:r#"(kind = ANY (ARRAY['agent'::text, 'model'::text, 'tool'::text, 'skill'::text, 'cluster'::text, 'node'::text, 'compactor'::text, 'embedding'::text]))"#.into() } })
        .add_operation(Operation::AddConstraintDefinition { table:"registry".into(),constraint:Constraint::Check { name:"registry_kind_check".into(),expression:"kind IN ('agent','model','tool','bundle','memory','source','skill','cluster','node','compactor','embedding')".into() } })
        .add_operation(Operation::AddConstraintDefinition { table:"registry".into(),constraint:Constraint::EnumDomain { name:"registry_kind_model_enum_check".into(),column:"kind".into(),domain:FieldDomain::Enum { repr:ModelEnumRepr::String,values:vec![ModelEnumValue::String("agent".into()),ModelEnumValue::String("bundle".into()),ModelEnumValue::String("cluster".into()),ModelEnumValue::String("compactor".into()),ModelEnumValue::String("embedding".into()),ModelEnumValue::String("memory".into()),ModelEnumValue::String("model".into()),ModelEnumValue::String("node".into()),ModelEnumValue::String("skill".into()),ModelEnumValue::String("source".into()),ModelEnumValue::String("tool".into())] } } })
        .add_operation(Operation::RunSQL {
            sql: include_str!("sql/forward/0008_provider_descriptors.sql").to_owned(),
            reverse_sql: Some(include_str!("sql/backward/0008_provider_descriptors.sql").to_owned()),
        })
}
