//! Agent Drive operations derive their scope from admitted executions.
use super::{index, jobs, repository as db, transfer};
use crate::apps::execution::capabilities::serializers::contracts::{Representation, SearchMode};
use crate::{Error, Result, authorization::access::Access, store::Store};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::{Run, nodes::NodeName};
use reinhardt::query::{Alias, Expr, ExprTrait, Order};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

pub(super) const MAINTENANCE: TenancyScope =
	TenancyScope::Deployment(DeploymentScopeReason::CapabilityMaintenance);

pub(crate) fn handles(name: &str, input: &Value) -> bool {
	matches!(
		name,
		"drive_publish" | "drive_publish_status" | "drive_search"
	) || name == "file_read" && input.get("drive").is_some_and(|value| !value.is_null())
		|| name == "file_search" && input["scope"] == "drive"
}

pub(crate) use super::index::Search;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Status {
	pub job_id: Uuid,
}

pub(crate) async fn invoke(
	store: &Store,
	mut access: Access,
	run: &Run,
	name: &str,
	input: Value,
	key: &str,
) -> Result<Value> {
	if !run.is_local(store.deployment.as_str()) {
		return access.finish(Err(Error::Forbidden)).await;
	}
	if name == "drive_publish" {
		let input = serde_json::from_value(input)?;
		return jobs::publish(store, access, run, input, key).await;
	}
	let result = match name {
		"drive_search" => {
			index::search(
				store,
				&mut access,
				run,
				serde_json::from_value(input)?,
				true,
			)
			.await
		}
		"file_read" => read(store, &mut access, run, input).await,
		"file_search" => files(store, &mut access, run, input).await,
		"drive_publish_status" => {
			let input: Status = serde_json::from_value(input)?;
			jobs::status(&mut access, run, input.job_id).await
		}
		_ => Err(Error::Invalid("unknown Drive operation".into())),
	};
	access.finish(result).await
}

async fn read(store: &Store, access: &mut Access, run: &Run, input: Value) -> Result<Value> {
	#[derive(Deserialize)]
	#[serde(deny_unknown_fields)]
	struct Input {
		drive: aidash_domain::drive::FileLocation,
		representation: Option<Representation>,
		offset: Option<usize>,
		max_bytes: Option<usize>,
		expected_digest: Option<String>,
	}
	let input: Input = serde_json::from_value(input)?;
	let location = input.drive;
	let (drive, grant) = db::drive(access, &NodeName::parse(&location.node)?, false).await?;
	let entry = db::file(access, &drive, &grant, &location.path, location.version).await?;
	if input
		.expected_digest
		.as_ref()
		.is_some_and(|expected| expected != &entry.file.digest)
	{
		return Err(Error::Conflict("Drive file digest changed".into()));
	}
	db::record_reads(access, run, [entry.id]).await?;
	match input.representation.unwrap_or(Representation::Text) {
		Representation::Metadata => Ok(serde_json::to_value(entry.file)?),
		Representation::Text => {
			let offset = input.offset.unwrap_or(0) as u64;
			let size = entry.file.size as u64;
			let limit = input.max_bytes.unwrap_or(64 * 1024);
			if offset > size || !(1..=1024 * 1024).contains(&limit) {
				return Err(Error::Invalid("invalid Drive read range".into()));
			}
			let end = size.min(offset.saturating_add(limit as u64));
			let text = if end > offset {
				let mut reader = transfer::open(store, &grant, &entry, Some(offset..end)).await?;
				let mut bytes = Vec::new();
				reader.read_to_end(&mut bytes).await?;
				match std::str::from_utf8(&bytes) {
					Ok(text) => text.to_owned(),
					Err(error) if error.error_len().is_none() && error.valid_up_to() > 0 => {
						String::from_utf8_lossy(&bytes[..error.valid_up_to()]).into_owned()
					}
					Err(_) => {
						return Err(Error::Invalid(
							"Drive file is not readable as UTF-8 at that offset".into(),
						));
					}
				}
			} else {
				String::new()
			};
			let next = offset + text.len() as u64;
			Ok(
				json!({"file":entry.file, "content":text, "encoding":"utf8", "next_offset": (next < size).then_some(next)}),
			)
		}
		Representation::ModelInput => {
			if input.expected_digest.as_deref() != Some(entry.file.digest.as_str())
				|| !entry.file.media_type.starts_with("image/")
				|| entry.file.size > 8 * 1024 * 1024
			{
				return Err(Error::Invalid("model_input requires an image and its exact digest within the model input limit".into()));
			}
			Ok(
				json!({"status": "completed", "metadata": entry.file, "selection": {
					"drive_entry": entry.id, "expected_digest": entry.file.digest
				}}),
			)
		}
	}
}

async fn files(store: &Store, access: &mut Access, run: &Run, input: Value) -> Result<Value> {
	use crate::apps::execution::capabilities::serializers::contracts::ToolFileSearch;
	let input: ToolFileSearch = serde_json::from_value(input)?;
	let limit = input.limit.unwrap_or(50);
	if input.query.len() > 1024 || !(1..=100).contains(&limit) || input.cursor.is_some() {
		return Err(Error::Invalid(
			"invalid Drive file search limit, query or cursor".into(),
		));
	}
	let matcher = match input.mode {
		SearchMode::Regex => Some(
			regex::Regex::new(&input.query).map_err(|error| Error::Invalid(error.to_string()))?,
		),
		_ => None,
	};
	let tenant = access.identity.tenant.clone();
	let query = db::select("drives", &tenant)
		.order_by(Alias::new("node"), Order::Asc)
		.to_owned();
	let drives = db::rows::<db::Drive>(access.tx.as_mut(), &query).await?;
	let mut matches = Vec::new();
	let mut reads = BTreeSet::new();
	let mut scanned = 0_u64;
	for drive in drives {
		let grant = match db::grant(access, &NodeName::parse(&drive.node)?).await {
			Ok(grant) if grant.authority().reads(drive.closed, "") => grant,
			Ok(_) | Err(Error::NotFound(_) | Error::Forbidden) => continue,
			Err(error) => return Err(error),
		};
		let mut query = db::snapshot(&drive, &grant.tenant, drive.head)?;
		db::readable(&mut query, &grant);
		if let Some(prefix) = &input.path {
			let prefix = prefix.trim_matches('/');
			if !prefix.is_empty() {
				aidash_domain::registry::rules::validate_path(prefix)?;
				query.cond_where(
					reinhardt::query::Condition::any()
						.add(db::col("path").eq(Expr::value(prefix)))
						.add(db::col("path").like(format!("{}/%", db::escape_like(prefix)))),
				);
			}
		}
		query.order_by(Alias::new("path"), Order::Asc);
		let glob = input.glob.as_deref().map(simple_glob).transpose()?;
		let mut readable = BTreeMap::new();
		for entry in db::rows::<db::Entry>(access.tx.as_mut(), &query).await? {
			let version = entry.file.added_in_version;
			if !readable.contains_key(&version) {
				let allowed = db::producer_readable(access, &tenant, drive.id, version).await?;
				readable.insert(version, allowed);
			}
			if !readable[&version] {
				continue;
			}
			let path = &entry.file.path;
			if glob.as_ref().is_some_and(|glob| !glob.is_match(path)) {
				continue;
			}
			let location =
				json!({"node": drive.node, "path": path, "version": entry.file.added_in_version});
			if matches!(input.mode, SearchMode::Path) {
				if path.contains(&input.query) {
					reads.insert(entry.id);
					matches.push(
						json!({"drive":location,"digest":entry.file.digest,"size":entry.file.size}),
					);
				}
			} else if (1..=1024 * 1024).contains(&entry.file.size) {
				scanned += entry.file.size as u64;
				if scanned > 16 * 1024 * 1024 {
					db::record_reads(access, run, reads).await?;
					return Ok(json!({"matches":matches,"truncated":true}));
				}
				let mut reader = transfer::open(store, &grant, &entry, None).await?;
				let mut data = Vec::new();
				reader.read_to_end(&mut data).await?;
				if let Ok(text) = std::str::from_utf8(&data) {
					for (line, body) in text.lines().enumerate() {
						if matcher.as_ref().map_or_else(
							|| body.contains(&input.query),
							|regex| regex.is_match(body),
						) {
							reads.insert(entry.id);
							matches.push(json!({"drive":location,"digest":entry.file.digest,"line":line+1,"text":body.chars().take(2000).collect::<String>()}));
							if matches.len() == limit {
								db::record_reads(access, run, reads).await?;
								return Ok(json!({"matches":matches,"truncated":true}));
							}
						}
					}
				}
			}
			if matches.len() == limit {
				db::record_reads(access, run, reads).await?;
				return Ok(json!({"matches":matches,"truncated":true}));
			}
		}
	}
	db::record_reads(access, run, reads).await?;
	Ok(json!({"matches":matches,"truncated":false}))
}

pub(crate) async fn model_file(
	store: &Store,
	access: &mut Access,
	selection: &aidash_domain::media::DriveFileSelection,
	remaining: u64,
) -> Result<(String, String, Vec<u8>)> {
	if !db::source_visible(access, selection.drive_entry).await? {
		return Err(Error::Forbidden);
	}
	let tenant = access.identity.tenant.clone();
	let query = db::select("drive_entries", &tenant)
		.and_where(db::col("id").eq(Expr::value(selection.drive_entry)))
		.to_owned();
	let file = db::rows::<db::Entry>(access.tx.as_mut(), &query)
		.await?
		.pop()
		.ok_or_else(|| Error::NotFound("Drive file not found".into()))?
		.file;
	if file.digest != selection.expected_digest
		|| !file.media_type.starts_with("image/")
		|| file.size as u64 > remaining
	{
		return Err(Error::Conflict(
			"Drive model image changed or exceeds the model input limit".into(),
		));
	}
	let digest = file
		.digest
		.parse()
		.map_err(|_| Error::Conflict("OBJECT_INTEGRITY".into()))?;
	let blobs = store.capabilities.blob_store().await?;
	let mut reader = blobs
		.get(&tenant, digest, None)
		.await?
		.ok_or_else(|| Error::NotFound("Drive file bytes not found".into()))?;
	let mut bytes = Vec::new();
	reader.read_to_end(&mut bytes).await?;
	Ok((file.path, file.media_type, bytes))
}

fn simple_glob(pattern: &str) -> Result<regex::Regex> {
	if pattern.len() > 1024 {
		return Err(Error::Invalid("Drive glob exceeds 1024 bytes".into()));
	}
	let mut regex = String::from("^");
	let mut chars = pattern.chars().peekable();
	while let Some(ch) = chars.next() {
		match ch {
			'*' if chars.next_if_eq(&'*').is_some() => regex.push_str(".*"),
			'*' => regex.push_str("[^/]*"),
			'?' => regex.push_str("[^/]"),
			ch => regex.push_str(&regex::escape(&ch.to_string())),
		}
	}
	regex.push('$');
	regex::Regex::new(&regex).map_err(|error| Error::Invalid(error.to_string()))
}
