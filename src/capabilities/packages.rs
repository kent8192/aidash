//! Offline wheel installation. Only broker-fetched, policy-approved artifacts
//! may enter the installer. No pip indexes, credentials, source builds, or
//! implicit dependency downloads cross the execution boundary.
use super::{approvals, contracts::*, operations, records, service};
use crate::{Error, Result, authorization::access::Access, domain::Run, store::Store};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Wheel {
	pub outbound_operation_id: Uuid,
	pub filename: String,
	pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Install {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	pub wheels: Vec<Wheel>,
	pub timeout_seconds: Option<u64>,
}
pub(crate) async fn authorize(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &Area,
	wheels: &[Wheel],
) -> Result<Vec<FileEntry>> {
	if wheels.is_empty() || wheels.len() > 16 {
		return Err(Error::Invalid("PACKAGE_SET_LIMIT".into()));
	}
	let mut entries = vec![];
	let mut names = std::collections::BTreeSet::new();
	for wheel in wheels {
		if wheel.filename.len() > 240
			|| !wheel.filename.ends_with(".whl")
			|| !wheel
				.filename
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
			|| !names.insert(&wheel.filename)
			|| wheel.sha256.len() != 64
			|| !wheel.sha256.bytes().all(|b| b.is_ascii_hexdigit())
		{
			return Err(Error::Invalid("INVALID_WHEEL_IDENTITY".into()));
		}
		let record = records::get(access, wheel.outbound_operation_id, "outbound").await?;
		if record.owner != access.identity.subject
			|| record.area_id != Some(area.id)
			|| record.data["run_id"] != json!(run.id)
			|| record.data["subjects"] != json!(access.subjects)
			|| record.state != "completed"
			|| record.data["http_status"] != 200
		{
			return Err(Error::NotFound("package artifact unavailable".into()));
		}
		let (url, origin) = approvals::permitted_origin(
			store,
			record.data["final_url"].as_str().ok_or(Error::Forbidden)?,
		)?;
		if !store.capabilities.0.package_origins.contains(&origin)
			|| url.path_segments().and_then(|mut p| p.next_back()) != Some(wheel.filename.as_str())
		{
			return Err(Error::Forbidden);
		}
		access
			.require(
				&access.resource(
					"package_source",
					&origin,
					json!({"run_id":run.id,"agent_id":run.agent_id,"agent_version":run.agent_version}),
				),
				"python.install",
			)
			.await?;
		let mut file: FileEntry = serde_json::from_value(record.data["output_file"].clone())?;
		if file.digest != wheel.sha256 {
			return Err(Error::Conflict("PACKAGE_DIGEST_CHANGED".into()));
		}
		file.path = format!(".packages/{}/{}", record.id, wheel.filename);
		file.scope = FileScope::References;
		entries.push(file);
	}
	Ok(entries)
}
pub(crate) async fn prepare(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &mut Area,
	input: Install,
) -> Result<Value> {
	let files = authorize(store, access, run, area, &input.wheels).await?;
	let sources = input.wheels.iter().zip(&files).map(|(wheel,file)| json!({"path":format!("/references/{}",file.path),"sha256":wheel.sha256,"filename":wheel.filename,"outbound_operation_id":wheel.outbound_operation_id})).collect::<Vec<_>>();
	let payload = STANDARD.encode(serde_json::to_vec(
		&json!({"wheels":sources,"image":store.capabilities.0.runner.as_ref().map(|p|&p.image)}),
	)?);
	let command = format!("python -I /opt/aidash/install.py '{payload}'");
	operations::prepare_kind(
		store,
		access,
		run,
		area,
		Shell {
			idempotency_key: input.idempotency_key,
			expected_revision: input.expected_revision,
			command,
			timeout_seconds: Some(
				input
					.timeout_seconds
					.unwrap_or(store.capabilities.0.install_seconds),
			),
		},
		"python_install",
		json!({"package_request":input,"package_files":files}),
	)
	.await
}
pub(crate) fn inputs(operation: &operations::Operation) -> Result<Vec<FileEntry>> {
	Ok(serde_json::from_value(
		operation
			.input
			.get("package_files")
			.cloned()
			.unwrap_or(json!([])),
	)?)
}
pub(crate) async fn environment(store: &Store, access: &mut Access, area: &Area) -> Result<Value> {
	let manifest = service::files(area)?.into_iter().find(|f| {
		matches!(f.scope, FileScope::Working) && f.path == ".aidash-python/manifest.json"
	});
	let dependencies = if let Some(file) = &manifest {
		if file.size > 65536 {
			return Err(Error::Conflict("PACKAGE_MANIFEST_LIMIT".into()));
		}
		Some(
			serde_json::from_slice::<Value>(&store.capabilities.read(access, file).await?)
				.map_err(|_| Error::Conflict("PACKAGE_MANIFEST_INVALID".into()))?,
		)
	} else {
		None
	};
	Ok(
		json!({"image":store.capabilities.0.runner.as_ref().map(|p|&p.image),"overlay_manifest":manifest,"dependencies":dependencies,"automatic_reinstall":false}),
	)
}
