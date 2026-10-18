//! Mapping writes own their transaction; inbound reads hold the resolved credential and policy lease.
use crate::Result;
use crate::tenancy::TenancyScope;
use aidash_domain::identity::peer_mapping::{Mapping, PeerMappingInput};
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
	async fn current(&mut self, node: &str, tenant: &str, subject: &str)
	-> Result<Option<Mapping>>;
	async fn peer_enabled(&mut self, node: &str) -> Result<bool>;
	fn environment(&mut self) -> &mut Value;
	fn context(&mut self) -> &mut Value;
}
#[async_trait]
pub trait MappingRepository: Send + Sync {
	type Write: MappingWrite;
	type Access: MappingAccess;
	fn node_id(&self) -> &str;
	async fn enabled_peer(&self, node: &str) -> Result<()>;
	async fn begin_write(&self, scope: &TenancyScope, enabled: bool) -> Result<Self::Write>;
	async fn resolved(&self, node: &str, tenant: &str, subject: &str) -> Result<Option<Mapping>>;
	async fn credential_subject(&self, mapping: &Mapping) -> Result<Option<String>>;
	async fn begin_access(
		&self,
		mapping: &Mapping,
		subject: &str,
		exclusive: bool,
	) -> Result<Self::Access>;
}
