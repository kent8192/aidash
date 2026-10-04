//! Offline installation requires the exact approved source, owner, run and digest.
use crate::{Error, Result, ports::capabilities::packages::PackageScope};
use aidash_domain::{
	RunMetadata,
	capabilities::{
		operations::{FileScope, MountedFile as FileEntry, ShellRequest as Shell},
		outbound::permitted_origin,
		packages::*,
		sessions::Area,
	},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
fn files(area: &Area) -> Result<Vec<FileEntry>> {
	Ok(serde_json::from_value(area.manifest.clone())?)
}
pub async fn authorize(
	scope: &mut dyn PackageScope,
	run: &RunMetadata,
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
		let record = scope.outbound(wheel.outbound_operation_id).await?;
		if record.owner != scope.principal()
			|| record.area_id != Some(area.id)
			|| record.data["run_id"] != json!(run.id)
			|| record.data["subjects"] != json!(scope.subjects())
			|| record.state != "completed"
			|| record.data["http_status"] != 200
		{
			return Err(Error::NotFound("package artifact unavailable".into()));
		}
		let (url, origin) = permitted_origin(
			record.data["final_url"].as_str().ok_or(Error::Forbidden)?,
			&scope.limits()?.outbound_origins,
		)?;
		if !scope.limits()?.package_origins.contains(&origin)
			|| url.path_segments().and_then(|mut p| p.next_back()) != Some(wheel.filename.as_str())
		{
			return Err(Error::Forbidden);
		}
		scope
			.require(
				&scope.resource(
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
pub async fn prepare(
	scope: &mut dyn PackageScope,
	run: &RunMetadata,
	area: &mut Area,
	input: Install,
) -> Result<Value> {
	let files = authorize(scope, run, area, &input.wheels).await?;
	let sources = input.wheels.iter().zip(&files).map(|(wheel,file)| json!({"path":format!("/references/{}",file.path),"sha256":wheel.sha256,"filename":wheel.filename,"outbound_operation_id":wheel.outbound_operation_id})).collect::<Vec<_>>();
	let payload = STANDARD.encode(serde_json::to_vec(
		&json!({"wheels":sources,"image":scope.limits()?.image}),
	)?);
	let command = format!("python -I /opt/aidash/install.py '{payload}'");
	scope
		.prepare(
			area,
			Shell {
				idempotency_key: input.idempotency_key,
				expected_revision: input.expected_revision,
				command,
				timeout_seconds: Some(
					input
						.timeout_seconds
						.unwrap_or(scope.limits()?.install_seconds),
				),
			},
			"python_install",
			json!({"package_request":input,"package_files":files}),
		)
		.await
}
pub async fn environment(scope: &mut dyn PackageScope, area: &Area) -> Result<Value> {
	let manifest = files(area)?.into_iter().find(|f| {
		matches!(f.scope, FileScope::Working) && f.path == ".aidash-python/manifest.json"
	});
	let dependencies = if let Some(file) = &manifest {
		if file.size > 65536 {
			return Err(Error::Conflict("PACKAGE_MANIFEST_LIMIT".into()));
		}
		Some(
			serde_json::from_slice::<Value>(&scope.read(file).await?)
				.map_err(|_| Error::Conflict("PACKAGE_MANIFEST_INVALID".into()))?,
		)
	} else {
		None
	};
	Ok(
		json!({"image":scope.limits()?.image,"overlay_manifest":manifest,"dependencies":dependencies,"automatic_reinstall":false}),
	)
}
