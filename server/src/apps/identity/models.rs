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
mod approval_preferences;
pub use approval_preferences::{
	ApprovalPreference, ApprovalRuleRecord, TenantApprovalSettings, ThreadApprovalMode,
};
mod dashboard_administration_history;
pub use dashboard_administration_history::DashboardAdministrationHistory;
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
mod dashboard_deployment_administrator_grants;
pub use dashboard_deployment_administrator_grants::DashboardDeploymentAdministratorGrant;
mod dashboard_registration_requests;
pub use dashboard_registration_requests::DashboardRegistrationRequest;
mod dashboard_sessions;
pub use dashboard_sessions::DashboardSession;
mod demo_data;
pub use demo_data::{DemoDataLoad, DemoReservation, DemoTenant};
mod demo_reservations;
pub use demo_reservations::{Fulfilment, Skip};
mod personal_tenants;
pub use personal_tenants::PersonalTenantLink;
mod tenant_invitations;
pub use tenant_invitations::TenantInvitation;
mod tenant_retention_rules;
pub use tenant_retention_rules::{TenantRetentionRule, TenantRetentionRuleHistory};
mod tenant_deletions;
pub use tenant_deletions::TenantDeletion;
mod tenants;
pub use tenants::Tenant;
mod nodes;
pub use nodes::{Node, NodeNameRoot};

mod dashboard_administration;
mod tenant_administration;
pub(crate) use dashboard_administration::database_time;
pub(crate) use tenant_administration::{
	AdministrationActor, AdministrationEvent, TenantAdministrator,
};
mod dashboard_authentication;
pub mod states;

mod authorization_remote_execution;
pub use authorization_remote_execution::AuthorizationRemoteExecution;

mod authorization_remote_commands;
pub use authorization_remote_commands::AuthorizationRemoteCommands;

mod authorization_remote_outputs;
pub use authorization_remote_outputs::AuthorizationRemoteOutputs;

pub mod byte_key;

mod desktop_handoffs;
pub use desktop_handoffs::DesktopHandoff;

mod desktop_refresh_credentials;
pub use desktop_refresh_credentials::DesktopRefreshCredential;

pub mod provider_credentials;

mod provider_authorizations;
pub use provider_authorizations::ProviderAuthorization;
pub mod plan_connections;

pub mod secret_store;

pub mod tenant_secrets;

mod tenant_data_protection;
pub use tenant_data_protection::{TenantDataProtection, TenantDataProtectionHistory};
