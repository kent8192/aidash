// reinhardt-migration-source: 1
// Native schema operations enforce Plan Connection ownership, catalog and state
// invariants (#159). Deleting a Plan Connection deletes its Run pins, so a Run
// admitted before a disconnection fails at its next call (ADR 0007); an index
// on the pins' referencing columns keeps that cascade from scanning them all.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0023_plan_connection_constraints", "identity")
.add_dependency("identity", "0022_plan_connections").database_only(true).atomic(true)
.add_operation(Operation::AddConstraintDefinition {table:"plan_connections".to_owned(),constraint:Constraint::Check {name:"plan_connection_catalog".to_owned(),expression:r#"provider = 'chatgpt'"#.to_owned()}})
.add_operation(Operation::AddConstraintDefinition {table:"plan_connections".to_owned(),constraint:Constraint::Check {name:"plan_connection_state".to_owned(),expression:r#"state IN ('active','needs_reconnection','ineligible') AND (state <> 'active' OR pinned_version IS NOT NULL)"#.to_owned()}})
.add_operation(Operation::AddConstraintDefinition {table:"plan_connections".to_owned(),constraint:Constraint::Check {name:"plan_connection_metadata".to_owned(),expression:r#"revision > 0 AND id::text ~ '^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$' AND secret_resource = 'aidash-store/plan-connections/' || id::text AND length(tenant) BETWEEN 1 AND 256 AND length(subject) BETWEEN 1 AND 256 AND length(client_id) BETWEEN 1 AND 256 AND length(account) BETWEEN 1 AND 512"#.to_owned()}})
.add_operation(Operation::AddConstraintDefinition {table:"plan_connections".to_owned(),constraint:Constraint::Check {name:"plan_connection_pin".to_owned(),expression:r#"pinned_version IS NULL OR (pinned_version ~ '/versions/[1-9][0-9]*$' AND left(pinned_version,length(secret_resource)+10) = secret_resource || '/versions/')"#.to_owned()}})
.add_operation(Operation::AddConstraintDefinition {table:"plan_connections".to_owned(),constraint:Constraint::Unique {name:"plan_connection_owner".to_owned(),columns:vec!["tenant".to_owned(),"subject".to_owned(),"provider".to_owned()]}})
.add_operation(Operation::AddConstraintDefinition {table:"plan_connections".to_owned(),constraint:Constraint::Unique {name:"plan_connection_owner_id".to_owned(),columns:vec!["tenant".to_owned(),"subject".to_owned(),"provider".to_owned(),"id".to_owned()]}})
.add_operation(Operation::AddConstraintDefinition {table:"run_plan_connections".to_owned(),constraint:Constraint::Check {name:"run_plan_connection_identity".to_owned(),expression:r#"id = run_id::text || ':' || provider AND provider = 'chatgpt'"#.to_owned()}})
.add_operation(Operation::AddConstraintDefinition {table:"run_plan_connections".to_owned(),constraint:Constraint::ForeignKey {name:"run_plan_connections_owner_target".to_owned(),columns:vec!["tenant".to_owned(),"subject".to_owned(),"provider".to_owned(),"plan_connection_id".to_owned()],referenced_table:"plan_connections".to_owned(),referenced_columns:vec!["tenant".to_owned(),"subject".to_owned(),"provider".to_owned(),"id".to_owned()],on_delete:ForeignKeyAction::Cascade,on_update:ForeignKeyAction::NoAction,deferrable:None}})
// PostgreSQL does not index a foreign key's referencing columns, and the
// cascade from deleting a connection looks its Run pins up by them.
// Every plan call counts the identities mapped to its owner by tenant and
// subject, which the mapping key, led by identity_id, cannot serve.
.add_operation(Operation::CreateIndex {table:"dashboard_mappings".into(),columns:vec!["tenant".into(),"subject".into()],unique:false,index_type:Some(IndexType::BTree),where_clause:Some("enabled".into()),concurrently:false,expressions:None,mysql_options:None,operator_class:None})
.add_operation(Operation::CreateIndex {table:"run_plan_connections".into(),columns:vec!["tenant".into(),"subject".into(),"provider".into(),"plan_connection_id".into()],unique:false,index_type:Some(IndexType::BTree),where_clause:None,concurrently:false,expressions:None,mysql_options:None,operator_class:None})
.add_operation(Operation::AddConstraintDefinition {table:"plan_authorizations".to_owned(),constraint:Constraint::Check {name:"plan_authorization_identity".to_owned(),expression:r#"provider = 'chatgpt' AND state_hash ~ '^[0-9a-f]{64}$' AND redirect_uri ~ '^http://127\.0\.0\.1:[0-9]{1,5}/auth/callback$'"#.to_owned()}})
.add_operation(Operation::AddConstraintDefinition {table:"plan_connection_hosts".to_owned(),constraint:Constraint::Check {name:"plan_connection_host_singleton".to_owned(),expression:r#"id = 'deployment'"#.to_owned()}})
}
