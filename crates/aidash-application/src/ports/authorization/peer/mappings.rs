//! Mapping writes own their transaction; inbound reads hold the resolved credential and policy lease.
use crate::Result;
use crate::tenancy::TenancyScope;
use aidash_domain::{
	identity::peer_mapping::{Mapping, PeerMappingInput},
	nodes::{DeploymentId, NodeAddress, NodeName},
	tenancy::TenantName,
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait MappingWrite: Send + Sized {
	async fn policy(&mut self, tenant: &str) -> Result<()>;
	async fn credential_subject(&mut self, tenant: &str, id: Uuid) -> Result<Option<String>>;
	/// Decides whether the writer may set `input`, whose credential acts as
	/// `subject`, in the same transaction as the write.
	async fn authorize(
		&mut self,
		_tenant: &str,
		_input: &PeerMappingInput,
		_subject: &str,
	) -> Result<()> {
		Ok(())
	}
	/// Requires `source` to be registered and enabled at `node`, read in this
	/// transaction once the writer is authorized, so a refused writer learns
	/// nothing of it.
	async fn enabled_registration(
		&mut self,
		tenant: &str,
		node: &NodeName,
		source: &NodeAddress,
	) -> Result<()>;
	/// Refuses a credential no Peer Mapping may bind, whether or not it is
	/// enabled.
	async fn exportable(&mut self, tenant: &str, id: Uuid) -> Result<()>;
	async fn lock_credential(&mut self, tenant: &str, id: Uuid, subject: &str) -> Result<()>;
	async fn insert(&mut self, tenant: &str, input: &PeerMappingInput) -> Result<Option<Mapping>>;
	async fn update(&mut self, tenant: &str, input: &PeerMappingInput) -> Result<Option<Mapping>>;
	async fn history(&mut self, mapping: &Mapping) -> Result<()>;
	async fn commit(self) -> Result<()>;
}
#[async_trait]
pub trait MappingAccess: Send {
	async fn current(
		&mut self,
		tenant: &TenantName,
		node: &NodeName,
		source: &NodeAddress,
		subject: &str,
	) -> Result<Option<Mapping>>;
	/// Whether `source` is still registered and enabled at `node`.
	async fn registered(
		&mut self,
		tenant: &TenantName,
		node: &NodeName,
		source: &NodeAddress,
	) -> Result<bool>;
	fn environment(&mut self) -> &mut Value;
	fn context(&mut self) -> &mut Value;
}
#[async_trait]
pub trait MappingRepository: Send + Sync {
	type Write: MappingWrite;
	type Access: MappingAccess;
	fn deployment(&self) -> &DeploymentId;
	async fn begin_write(&self, scope: &TenancyScope, enabled: bool) -> Result<Self::Write>;
	async fn resolved(
		&self,
		tenant: &TenantName,
		node: &NodeName,
		source: &NodeAddress,
		subject: &str,
	) -> Result<Option<Mapping>>;
	async fn credential_subject(&self, mapping: &Mapping) -> Result<Option<String>>;
	async fn begin_access(
		&self,
		mapping: &Mapping,
		subject: &str,
		exclusive: bool,
	) -> Result<Self::Access>;
	/// An authority for a subject of the Tenant's own policy that holds no
	/// credential: a Node of the same Tenant acting for one of its subjects.
	async fn begin_subject(
		&self,
		tenant: &TenantName,
		subject: &str,
		exclusive: bool,
	) -> Result<Self::Access>;
}
