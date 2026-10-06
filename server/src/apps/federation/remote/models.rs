//! Persistent records owned by the remote app.

mod authorization_remote_grant_reads;
pub use authorization_remote_grant_reads::AuthorizationRemoteGrantRead;
mod authorization_remote_grants;
pub use authorization_remote_grants::AuthorizationRemoteGrant;
mod delegations;
pub use delegations::Delegation;
mod remote_run_message_fences;
pub use remote_run_message_fences::RemoteRunMessageFence;

pub(crate) mod input_fences;
