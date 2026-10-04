//! Package adapters preserve the borrowed operation transaction and immutable artifact codecs.
use super::capability_records::domain;
use crate::apps::execution::capabilities::{
	serializers::{
		contracts::{Area as NativeArea, FileEntry},
		packages::{Install as NativeInstall, Wheel as NativeWheel},
	},
	services::{operations, records},
};
use crate::{Result as NativeResult, authorization::access::Access, domain::Run, store::Store};
use aidash_application::{
	Error, Result,
	ports::capabilities::packages::{Limits, PackageScope},
};
use aidash_domain::{
	capabilities::{
		operations::{MountedFile, ShellRequest},
		packages::{Install, Wheel},
		records::Record,
		sessions::Area,
	},
	policy::Resource,
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Scope<'a> {
	pub(crate) store: &'a Store,
	pub(crate) access: &'a mut Access,
	pub(crate) run: Option<&'a Run>,
}
impl From<NativeWheel> for Wheel {
	fn from(v: NativeWheel) -> Self {
		Self {
			outbound_operation_id: v.outbound_operation_id,
			filename: v.filename,
			sha256: v.sha256,
		}
	}
}
impl From<NativeInstall> for Install {
	fn from(v: NativeInstall) -> Self {
		Self {
			idempotency_key: v.idempotency_key,
			expected_revision: v.expected_revision,
			wheels: v.wheels.into_iter().map(Into::into).collect(),
			timeout_seconds: v.timeout_seconds,
		}
	}
}
#[async_trait]
impl PackageScope for Scope<'_> {
	fn principal(&self) -> &str {
		&self.access.identity.subject
	}
	fn subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn limits(&self) -> Result<Limits> {
		let p = &self.store.capabilities.0;
		Ok(Limits {
			outbound_origins: p.outbound_origins.clone(),
			package_origins: p.package_origins.clone(),
			image: p.runner.as_ref().map(|p| p.image.clone()),
			install_seconds: p.install_seconds,
		})
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn outbound(&mut self, id: Uuid) -> Result<Record> {
		records::get(self.access, id, "outbound")
			.await
			.map(domain)
			.map_err(Into::into)
	}
	async fn read(&mut self, file: &MountedFile) -> Result<Vec<u8>> {
		let row: FileEntry = file.clone().into();
		self.store
			.capabilities
			.read(self.access, &row)
			.await
			.map_err(Into::into)
	}
	async fn prepare(
		&mut self,
		area: &mut Area,
		input: ShellRequest,
		kind: &str,
		extra: Value,
	) -> Result<Value> {
		let mut row: NativeArea = area.clone().into();
		let result: NativeResult<Value> = operations::prepare_kind(
			self.store,
			self.access,
			self.run
				.ok_or_else(|| Error::External("package repository scope invariant".into()))?,
			&mut row,
			input.into(),
			kind,
			extra,
		)
		.await;
		*area = row.into();
		result.map_err(Into::into)
	}
}
