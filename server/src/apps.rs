//! Application registry for server
//!
//! This file maintains the list of installed apps.
//! New apps created with `startapp` will be automatically added here.
pub mod identity;
pub use identity::IdentityConfig;
pub mod registry;
pub use registry::RegistryConfig;
pub mod workspaces;
pub use workspaces::WorkspacesConfig;
pub mod execution;
pub use execution::ExecutionConfig;
pub mod federation;
pub use federation::FederationConfig;
pub mod knowledge;
pub use knowledge::KnowledgeConfig;
pub mod marketplace;
pub use marketplace::MarketplaceConfig;
pub mod operations;
pub use operations::OperationsConfig;
