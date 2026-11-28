//! Drive-specific passages. The manifest and current authority own visibility.
use super::repository as db;
use crate::{Error, Result, apps::identity::repositories::embedding_settings, store::Store};
use aidash_application::{
	embedding::{EmbeddingRef, tenant_embedding},
	ports::EmbeddingProvider,
	provider_access::MaintenancePurpose,
	tenancy::{DeploymentScopeReason, TenancyScope},
};
use aidash_domain::semantic::{EmbeddingConfig, EmbeddingInput, EmbeddingInputKind};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoValue, LockType, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tokio::io::AsyncReadExt;
use uuid::Uuid;

const CONFIRM_MICROS: u64 = 10_000_000;
const MAX_EXTRACT_BYTES: u64 = 32 * 1024 * 1024;
const MAINTENANCE: TenancyScope =
	TenancyScope::Deployment(DeploymentScopeReason::SemanticMaintenance);

fn embedding_key(reference: &EmbeddingRef) -> String {
	format!("{}@{}", reference.id, reference.version)
}

async fn config(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
) -> Result<(String, EmbeddingConfig)> {
	let reference = tenant_embedding(
		&embedding_settings::EmbeddingSettings(tokio::sync::Mutex::new(tx)),
		tenant,
	)
	.await?;
	config_for_reference(tx, tenant, &reference).await
}

async fn config_for_reference(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	reference: &EmbeddingRef,
) -> Result<(String, EmbeddingConfig)> {
	Ok((
		embedding_key(reference),
		embedding_settings::embedding_config(tx, tenant, reference).await?,
	))
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(rename = "DriveIndexConfirmation")]
pub struct Request {
	pub id: Uuid,
	pub version: i64,
	pub estimated_micros: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct Pending {
	id: Uuid,
	tenant: String,
	drive_id: Uuid,
	version: i64,
	state: String,
}

pub(super) async fn confirmations(
	tx: &mut dyn TransactionExecutor,
	drive: Uuid,
	grant: &db::Grant,
) -> Result<Vec<Request>> {
	let mut query = db::select("drive_index_requests", &grant.tenant);
	query
		.and_where(db::col("drive_id").eq(Expr::value(drive)))
		.and_where(db::col("requester").eq(Expr::value(&grant.person)))
		.and_where(db::col("state").eq(Expr::value("awaiting_confirmation")));
	db::rows(tx, &query).await
}

pub(super) async fn confirm(
	tx: &mut dyn TransactionExecutor,
	drive: Uuid,
	grant: &db::Grant,
	id: Uuid,
) -> Result<()> {
	if !confirmations(tx, drive, grant)
		.await?
		.iter()
		.any(|request| request.id == id)
	{
		return Err(Error::NotFound("index confirmation not found".into()));
	}
	db::update(
		tx,
		"drive_index_requests",
		&grant.tenant,
		id,
		[("state", Expr::value("confirmed").into())],
	)
	.await
}

pub(super) async fn tick(store: &Store) -> Result<()> {
	let queue = Query::select()
		.expr(Expr::cust("*"))
		.from(Alias::new("drive_index_requests"))
		.and_where(db::col("state").is_in(["pending", "confirmed"]))
		.order_by(Alias::new("version"), Order::Asc)
		.limit(16)
		.to_owned();
	let requests = store
		.pool
		.atomic(&MAINTENANCE, async |tx| {
			db::rows::<Pending>(tx, &queue).await
		})
		.await?;
	for request in requests {
		if let Err(error) = process(store, &request).await {
			tracing::warn!(request = %request.id, %error, "Drive indexing will retry");
		}
	}
	rebuild_pending(store).await
}

async fn process(store: &Store, pending: &Pending) -> Result<()> {
	let tenant = &pending.tenant;
	let scope = TenancyScope::tenant(tenant)?;
	let mut tx = store.pool.begin(&scope).await?;
	// Serialize the active generation with Tenant embedding promotion.
	let tenant_fence = Query::select()
		.expr(Expr::value(1_i32))
		.from(Alias::new("tenants"))
		.and_where(db::col("name").eq(Expr::value(tenant)))
		.lock(LockType::Share)
		.to_owned();
	let (sql, values) = tenant_fence.build(PostgresQueryBuilder);
	tx.as_mut()
		.fetch_optional(&sql, reinhardt::db::orm::execution::convert_values(values))
		.await?;
	let request = db::select("drive_index_requests", tenant)
		.and_where(db::col("id").eq(Expr::value(pending.id)))
		.and_where(db::col("state").eq(Expr::value(&pending.state)))
		.lock(LockType::Update)
		.to_owned();
	if db::rows::<Pending>(tx.as_mut(), &request).await?.is_empty() {
		return Ok(());
	}
	let (key, embedding) = config(tx.as_mut(), tenant).await?;
	let query = db::select("drive_entries", tenant)
		.and_where(db::col("drive_id").eq(Expr::value(pending.drive_id)))
		.and_where(db::col("added_in_version").eq(Expr::value(pending.version)))
		.and_where(db::col("indexed").eq(true))
		.and_where(db::col("removed_in_version").is_null())
		.to_owned();
	let entries = db::rows::<db::Entry>(tx.as_mut(), &query).await?;
	let mut missing = Vec::new();
	let mut seen = BTreeSet::new();
	for entry in entries {
		if !seen.insert(entry.file.digest.clone()) {
			continue;
		}
		if passages(tx.as_mut(), tenant, &entry.file.digest, &key)
			.await?
			.is_empty()
		{
			missing.push(entry);
		}
	}
	// Apply the built-in memory rate to what the provider will actually see.
	// Compressed documents can yield far more input tokens than their blob size.
	let estimate = if pending.state == "pending" {
		estimate_indexing(store, tenant, &embedding, &missing).await?
	} else {
		0
	};
	if estimate > CONFIRM_MICROS {
		db::update(
			tx.as_mut(),
			"drive_index_requests",
			tenant,
			pending.id,
			[
				("state", Expr::value("awaiting_confirmation").into()),
				(
					"estimated_micros",
					Expr::value(i64::try_from(estimate).unwrap_or(i64::MAX)).into(),
				),
			],
		)
		.await?;
		tx.commit().await?;
		return Ok(());
	}
	populate(store, tx.as_mut(), tenant, &key, &embedding, missing).await?;
	db::update(
		tx.as_mut(),
		"drive_index_requests",
		tenant,
		pending.id,
		[("state", Expr::value("ready").into())],
	)
	.await?;
	tx.commit().await?;
	Ok(())
}

async fn estimate_indexing(
	store: &Store,
	tenant: &str,
	embedding: &EmbeddingConfig,
	entries: &[db::Entry],
) -> Result<u64> {
	let mut tokens = 0_u64;
	for entry in entries {
		for body in extract(store, tenant, &entry.file, embedding).await? {
			let input = body.input(&entry.file.media_type);
			tokens = tokens.saturating_add(input.reservation_amount()? as u64);
		}
	}
	let rate = aidash_domain::memory::Rate {
		input_per_million: 1_000_000,
		output_per_million: 0,
	};
	Ok(rate.charge(tokens, 0)?)
}

async fn populate(
	store: &Store,
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	key: &str,
	embedding: &EmbeddingConfig,
	missing: Vec<db::Entry>,
) -> Result<()> {
	let transport = crate::bootstrap::admitted_semantic_transport(
		store,
		None,
		tenant.to_owned(),
		Some(MaintenancePurpose::MemoryIndexing),
	);
	for entry in missing {
		let file = &entry.file;
		let bodies = extract(store, tenant, file, embedding).await?;
		if bodies.is_empty() {
			record(tx, tenant, &file.digest, key, 0, None, vec![]).await?;
		}
		for (ordinal, body) in bodies.iter().enumerate() {
			let input = body.input(&file.media_type);
			let vector = transport.embed(embedding, input).await?.vector;
			record(
				tx,
				tenant,
				&file.digest,
				key,
				ordinal as i64,
				Some(body),
				vector,
			)
			.await?;
		}
	}
	Ok(())
}

fn entries_for_pending(tenant: &str, key: &str, limit: u64) -> reinhardt::query::SelectStatement {
	let entry = |name: &str| Expr::col((Alias::new("drive_entries"), Alias::new(name)));
	let ready = Query::select()
		.expr(Expr::value(1_i32))
		.from(Alias::new("drive_index_requests"))
		.and_where(db::col("tenant").eq(Expr::value(tenant)))
		.and_where(db::col("drive_id").eq(entry("drive_id")))
		.and_where(db::col("version").eq(entry("added_in_version")))
		.and_where(db::col("state").eq(Expr::value("ready")))
		.to_owned();
	let exists = Query::select()
		.expr(Expr::value(1_i32))
		.from(Alias::new("drive_passages"))
		.and_where(db::col("tenant").eq(Expr::value(tenant)))
		.and_where(db::col("digest").eq(entry("digest")))
		.and_where(db::col("embedding_key").eq(Expr::value(key)))
		.to_owned();
	db::select("drive_entries", tenant)
		.and_where(db::col("removed_in_version").is_null())
		.and_where(db::col("indexed").eq(true))
		.and_where(Expr::exists(ready))
		.and_where(Expr::not_exists(exists))
		.distinct_on([Alias::new("digest")])
		.order_by(Alias::new("digest"), Order::Asc)
		.limit(limit)
		.to_owned()
}

/// Whether ready Drive publications have been indexed with the pending Tenant model.
pub(crate) async fn reembedding_ready(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	pending: &EmbeddingRef,
) -> Result<bool> {
	Ok(
		db::rows::<db::Entry>(tx, &entries_for_pending(tenant, &embedding_key(pending), 1))
			.await?
			.is_empty(),
	)
}

async fn rebuild_pending(store: &Store) -> Result<()> {
	#[derive(Deserialize)]
	struct Generation {
		tenant: String,
		pending: EmbeddingRef,
	}
	let mut tx = store.pool.begin(&MAINTENANCE).await?;
	let query = Query::select()
		.columns(["tenant", "pending"].map(Alias::new))
		.from(Alias::new("tenant_embedding_settings"))
		.and_where(db::col("pending").is_not_null())
		.to_owned();
	let generations = db::rows::<Generation>(tx.as_mut(), &query).await?;
	tx.commit().await?;
	for generation in generations {
		let result = rebuild_generation(store, &generation.tenant, &generation.pending).await;
		if let Err(error) = result {
			tracing::warn!(tenant = %generation.tenant, %error, "Drive re-embedding will retry");
		}
	}
	Ok(())
}

async fn rebuild_generation(store: &Store, tenant: &str, reference: &EmbeddingRef) -> Result<()> {
	let mut tx = store.pool.begin(&TenancyScope::tenant(tenant)?).await?;
	let settings = db::select("tenant_embedding_settings", tenant)
		.lock(LockType::Update)
		.to_owned();
	#[derive(Deserialize)]
	struct PendingSetting {
		pending: Option<EmbeddingRef>,
	}
	let current = db::rows::<PendingSetting>(tx.as_mut(), &settings)
		.await?
		.pop()
		.and_then(|row| row.pending);
	if current.as_ref() != Some(reference) {
		return Ok(());
	}
	let (key, embedding) = config_for_reference(tx.as_mut(), tenant, reference).await?;
	let missing =
		db::rows::<db::Entry>(tx.as_mut(), &entries_for_pending(tenant, &key, 16)).await?;
	populate(store, tx.as_mut(), tenant, &key, &embedding, missing).await?;
	tx.commit().await?;
	Ok(())
}

#[derive(Deserialize)]
struct Passage {
	start_line: Option<i64>,
	end_line: Option<i64>,
	body: Option<String>,
	vector: Vec<f32>,
}

async fn passages(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	digest: &str,
	key: &str,
) -> Result<Vec<Passage>> {
	let query = db::select("drive_passages", tenant)
		.and_where(db::col("digest").eq(Expr::value(digest)))
		.and_where(db::col("embedding_key").eq(Expr::value(key)))
		.order_by(Alias::new("ordinal"), Order::Asc)
		.to_owned();
	db::rows(tx, &query).await
}

enum Content {
	Text { text: String, start: i64, end: i64 },
	Image(Vec<u8>),
}

impl Content {
	fn input<'a>(&'a self, media_type: &'a str) -> EmbeddingInput<'a> {
		match self {
			Self::Text { text, .. } => EmbeddingInput::Text(text),
			Self::Image(bytes) => EmbeddingInput::Image { media_type, bytes },
		}
	}
}

async fn record(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	digest: &str,
	key: &str,
	ordinal: i64,
	content: Option<&Content>,
	vector: Vec<f32>,
) -> Result<()> {
	let (start, end, text) = match content {
		Some(Content::Text { text, start, end }) => (Some(*start), Some(*end), Some(text.clone())),
		_ => (None, None, None),
	};
	let (sql, values) = Query::insert()
		.into_table(Alias::new("drive_passages"))
		.columns(
			[
				"id",
				"tenant",
				"digest",
				"embedding_key",
				"ordinal",
				"start_line",
				"end_line",
				"body",
				"vector",
			]
			.map(Alias::new),
		)
		.values_panic::<_, reinhardt::query::Value>([
			Uuid::new_v4().into(),
			tenant.into(),
			digest.into(),
			key.into(),
			ordinal.into(),
			start.into_value(),
			end.into_value(),
			text.into_value(),
			serde_json::to_value(vector)?.into(),
		])
		.on_conflict(
			OnConflict::columns(["tenant", "digest", "embedding_key", "ordinal"].map(Alias::new))
				.do_nothing()
				.to_owned(),
		)
		.build(PostgresQueryBuilder);
	tx.execute(&sql, reinhardt::db::orm::execution::convert_values(values))
		.await?;
	Ok(())
}

fn supported(media: &str, file: &str) -> bool {
	media.starts_with("text/")
		|| media.starts_with("image/")
		|| media == "application/pdf"
		|| matches!(
			file.rsplit('.').next(),
			Some(
				"json"
					| "rs" | "py" | "ts"
					| "tsx" | "js" | "jsx"
					| "toml" | "yaml"
					| "yml" | "sh" | "sql"
					| "xml" | "docx"
					| "xlsx" | "pptx"
			)
		)
}

async fn extract(
	store: &Store,
	tenant: &str,
	file: &aidash_domain::drive::File,
	config: &EmbeddingConfig,
) -> Result<Vec<Content>> {
	if !supported(&file.media_type, &file.path) || file.size as u64 > MAX_EXTRACT_BYTES {
		return Ok(vec![]);
	}
	let digest = file
		.digest
		.parse()
		.map_err(|_| Error::Conflict("OBJECT_INTEGRITY".into()))?;
	let blobs = store.capabilities.blob_store().await?;
	let reader = blobs
		.get(tenant, digest, None)
		.await?
		.ok_or_else(|| Error::NotFound("Drive content missing".into()))?;
	let mut bytes = Vec::new();
	reader
		.take(MAX_EXTRACT_BYTES)
		.read_to_end(&mut bytes)
		.await?;
	let text = if file.media_type.starts_with("image/") {
		return Ok(
			if config.inputs.contains(&EmbeddingInputKind::Image) && !bytes.is_empty() {
				vec![Content::Image(bytes)]
			} else {
				vec![]
			},
		);
	} else if file.media_type == "application/pdf" {
		use aidash_integrations::research::pdf::{ChildEngine, PdfEngine};
		match ChildEngine::current_exe()?.extract(bytes).await {
			Ok(pdf) => pdf
				.lines
				.into_iter()
				.map(|line| line.text)
				.collect::<Vec<_>>()
				.join("\n"),
			Err(_) => return Ok(vec![]),
		}
	} else if let Some(kind @ ("docx" | "xlsx" | "pptx")) = file.path.rsplit('.').next() {
		let kind = kind.to_owned();
		tokio::task::spawn_blocking(move || office_text(bytes, &kind))
			.await
			.map_err(|error| Error::External(error.to_string()))?
			.unwrap_or_default()
	} else {
		String::from_utf8(bytes).unwrap_or_default()
	};
	Ok(aidash_application::drive::passages(&text)
		.filter(|passage| !passage.text.trim().is_empty())
		.map(|passage| Content::Text {
			text: passage.text.into(),
			start: passage.start_line as i64,
			end: passage.end_line as i64,
		})
		.collect())
}

fn office_text(bytes: Vec<u8>, kind: &str) -> Option<String> {
	use quick_xml::events::Event;
	use std::io::{BufReader, Cursor, Read};
	let mut document = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
	let mut names: Vec<_> = document
		.file_names()
		.filter(|name| match kind {
			"docx" => *name == "word/document.xml",
			"xlsx" => {
				*name == "xl/sharedStrings.xml"
					|| name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml")
			}
			_ => name.starts_with("ppt/slides/slide") && name.ends_with(".xml"),
		})
		.map(str::to_owned)
		.collect();
	names.sort_unstable();
	let mut text = String::new();
	let mut budget = MAX_EXTRACT_BYTES;
	for name in names {
		let item = document.by_name(&name).ok()?;
		let size = item.size();
		budget = budget.checked_sub(size)?;
		let mut reader = quick_xml::Reader::from_reader(BufReader::new(item.take(size)));
		let mut buffer = Vec::new();
		let mut inside_text = false;
		loop {
			match reader.read_event_into(&mut buffer).ok()? {
				Event::Start(element) => {
					inside_text = matches!(element.local_name().as_ref(), "t" | "v")
				}
				Event::Text(value) if inside_text => text.push_str(value.as_ref()),
				Event::GeneralRef(reference) if inside_text => {
					if let Some(ch) = reference.resolve_char_ref().ok()? {
						text.push(ch);
					} else {
						let name = reference.as_ref();
						text.push_str(quick_xml::escape::resolve_predefined_entity(name)?);
					}
				}
				Event::End(element) => {
					inside_text = false;
					if matches!(element.local_name().as_ref(), "p" | "row" | "si") {
						text.push('\n');
					} else if element.local_name().as_ref() == "c" {
						text.push('\t');
					}
				}
				Event::Eof => break,
				_ => {}
			}
			buffer.clear();
		}
	}
	Some(text)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Search {
	pub query: String,
	pub node: Option<String>,
	pub limit: Option<usize>,
}

#[derive(Serialize)]
struct Hit {
	#[serde(skip)]
	entry: Uuid,
	node: String,
	path: String,
	version: i64,
	digest: String,
	start_line: Option<i64>,
	end_line: Option<i64>,
	text: Option<String>,
	score: f32,
}

pub(super) async fn search(
	store: &Store,
	access: &mut crate::authorization::access::Access,
	run: &aidash_domain::Run,
	input: Search,
) -> Result<serde_json::Value> {
	let limit = input.limit.unwrap_or(20);
	if input.query.len() > 4096 || input.query.trim().is_empty() || !(1..=100).contains(&limit) {
		return Err(Error::Invalid("invalid Drive search query or limit".into()));
	}
	let tenant = access.identity.tenant.clone();
	let mut query = db::select("drives", &tenant);
	if let Some(node) = &input.node {
		query.and_where(db::col("node").eq(Expr::value(node)));
	}
	let drives = db::rows::<db::Drive>(access.tx.as_mut(), &query).await?;
	let embedding = {
		// Name and path search remains available if no model is configured yet.
		match config(access.tx.as_mut(), &tenant).await {
			Ok((key, config)) => {
				let mut lease = crate::semantic::service::Lease::Inherited(access);
				let vector = crate::semantic::service::embed(
					store,
					&mut lease,
					run.thread_id,
					&config,
					&input.query,
					aidash_domain::generation::embedding::Origin::Query(Some(run.id)),
				)
				.await?;
				Some((key, vector))
			}
			Err(Error::NotFound(_)) => None,
			Err(error) => return Err(error),
		}
	};
	let query_lower = input.query.to_lowercase();
	let mut hits = Vec::new();
	for drive in drives {
		let node = aidash_domain::nodes::NodeName::parse(&drive.node)?;
		let grant = match db::grant(access, &node).await {
			Ok(grant) if grant.authority().reads(drive.closed, "") => grant,
			Ok(_) | Err(Error::NotFound(_) | Error::Forbidden) => continue,
			Err(error) => return Err(error),
		};
		let mut query = db::snapshot(&drive, &tenant, drive.head)?;
		db::readable(&mut query, &grant);
		{
			let entry = |name: &str| Expr::col((Alias::new("drive_entries"), Alias::new(name)));
			let approved = Query::select()
				.expr(Expr::value(1_i32))
				.from(Alias::new("drive_index_requests"))
				.and_where(db::col("tenant").eq(Expr::value(&tenant)))
				.and_where(db::col("drive_id").eq(entry("drive_id")))
				.and_where(db::col("version").eq(entry("added_in_version")))
				.and_where(db::col("state").eq(Expr::value("ready")))
				.to_owned();
			query
				.and_where(db::col("indexed").eq(true))
				.and_where(Expr::exists(approved));
		}

		let mut readable = std::collections::BTreeMap::new();
		for entry in db::rows::<db::Entry>(access.tx.as_mut(), &query).await? {
			let file = entry.file;
			let version = file.added_in_version;
			if !readable.contains_key(&version) {
				let allowed = db::producer_readable(access, &tenant, drive.id, version).await?;
				readable.insert(version, allowed);
			}
			if !readable[&version] {
				continue;
			}
			let hit = |score, start_line, end_line, text| Hit {
				entry: entry.id,
				node: drive.node.clone(),
				path: file.path.clone(),
				version: file.added_in_version,
				digest: file.digest.clone(),
				score,
				start_line,
				end_line,
				text,
			};
			let name_match = file.path.to_lowercase().contains(&query_lower);
			if name_match {
				hits.push(hit(1.0, None, None, None));
			}
			if let Some((key, vector)) = &embedding {
				for passage in passages(access.tx.as_mut(), &tenant, &file.digest, key).await? {
					if let Some(score) =
						cosine(vector, &passage.vector).filter(|score| *score > 0.0)
					{
						hits.push(hit(
							score,
							passage.start_line,
							passage.end_line,
							passage.body,
						));
					}
				}
			}
		}
	}
	hits.sort_by(|left, right| {
		right
			.score
			.total_cmp(&left.score)
			.then_with(|| left.path.cmp(&right.path))
	});
	hits.truncate(limit);
	db::record_reads(access, run, hits.iter().map(|hit| hit.entry)).await?;
	Ok(serde_json::json!({"matches": hits}))
}

fn cosine(left: &[f32], right: &[f32]) -> Option<f32> {
	if left.len() != right.len() || left.is_empty() {
		return None;
	}
	let (dot, l2, r2) = left
		.iter()
		.zip(right)
		.fold((0.0, 0.0, 0.0), |(dot, l2, r2), (l, r)| {
			(dot + l * r, l2 + l * l, r2 + r * r)
		});
	let score: f32 = dot / (l2 * r2).sqrt();
	score.is_finite().then_some(score)
}
