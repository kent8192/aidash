//! Persistent records owned by the authorization app.
mod authority;
mod read_dependencies;

mod authorization_bundles;
pub use authorization_bundles::AuthorizationBundle;
mod authorization_catalog;
pub use authorization_catalog::AuthorizationCatalog;
mod authorization_catalog_history;
pub use authorization_catalog_history::AuthorizationCatalogHistory;
mod authorization_credentials;
pub use authorization_credentials::AuthorizationCredential;
mod authorization_decisions;
pub use authorization_decisions::AuthorizationDecision;
mod authorization_execution;
pub use authorization_execution::AuthorizationExecution;
mod authorization_revisions;
pub use authorization_revisions::AuthorizationRevision;
mod authorization_run_outputs;
pub use authorization_run_outputs::AuthorizationRunOutput;
mod authorization_run_reads;
pub use authorization_run_reads::AuthorizationRunRead;
mod authorization_run_registry_reads;
pub use authorization_run_registry_reads::AuthorizationRunRegistryRead;
mod authorization_run_remote_reads;
pub use authorization_run_remote_reads::AuthorizationRunRemoteRead;
mod authorization_task_origins;
pub use authorization_task_origins::AuthorizationTaskOrigin;
mod authorization_workspaces;
pub use authorization_workspaces::AuthorizationWorkspace;
mod dashboard_execution_origins;
pub use dashboard_execution_origins::DashboardExecutionOrigin;
mod dashboard_identities;
pub use dashboard_identities::DashboardIdentity;
mod dashboard_login_transactions;
pub use dashboard_login_transactions::DashboardLoginTransaction;
mod dashboard_logout_tokens;
pub use dashboard_logout_tokens::DashboardLogoutToken;
mod dashboard_mappings;
pub use dashboard_mappings::DashboardMapping;
mod dashboard_operator_grants;
pub use dashboard_operator_grants::DashboardOperatorGrant;
mod dashboard_registration_requests;
pub use dashboard_registration_requests::DashboardRegistrationRequest;
mod dashboard_sessions;
pub use dashboard_sessions::DashboardSession;

mod dashboard_administration;
mod dashboard_authentication;
pub mod states;

mod authorization_remote_execution;
pub use authorization_remote_execution::AuthorizationRemoteExecution;

mod authorization_remote_commands;
pub use authorization_remote_commands::AuthorizationRemoteCommands;

mod authorization_remote_outputs;
pub use authorization_remote_outputs::AuthorizationRemoteOutputs;

mod authorization_graph_operator_grants;
pub use authorization_graph_operator_grants::AuthorizationGraphOperatorGrants;

pub(crate) use authority::authority_control;

pub mod byte_key;
