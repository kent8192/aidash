//! Preview persistence and the gateway's authority (ADR 0068).
//!
//! A Preview's subdomain and a ticket's digest are globally unique random
//! values, so the gateway resolves them in Deployment Scope before it opens
//! the viewer's Tenant Scope. Everything after that resolution is the same
//! authority check the Working Area download runs.
use crate::{
	Error as NativeError, Result as NativeResult,
	apps::execution::capabilities::{
		serializers::contracts::FileEntry,
		services::{retention, service, sessions},
	},
	apps::identity::services::identity::SubjectIdentity,
	authorization::access::Access,
	store::Store,
};
use aidash_application::preview::{
	self as application, Exchange, FileVersion, Page, Preview, TicketIssue,
};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::capabilities::retention::RetainedFile;
use aidash_domain::preview::{PreviewKind, PreviewLabel, PreviewSecret, label_of};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, IntoValue as _, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _,
};
use uuid::Uuid;

const PREVIEW_SCOPE: DeploymentScopeReason = DeploymentScopeReason::PreviewGateway;

type ResolvedRow = (Uuid, String, String, Option<Uuid>, Option<Uuid>);

/// The ticket use case's authority and transaction.
pub(crate) struct Scope<'a> {
	pub access: &'a mut Access,
	pub store: &'a Store,
	pub domain: Option<&'a str>,
	pub public_port: Option<u16>,
	pub origin: &'a str,
}

impl Scope<'_> {
	async fn authorize_file_native(
		&mut self,
		area_id: Uuid,
		file_id: Uuid,
	) -> NativeResult<FileVersion> {
		let area = sessions::load(self.access, area_id).await?;
		// The current manifest pins today's File version; a superseded version
		// opens only while a hold keeps its manifest (ADR 0095).
		let current = service::files(&area)?
			.into_iter()
			.find(|entry| entry.file_id == file_id);
		let entry = match current {
			Some(entry) => entry,
			None => {
				let retained =
					retention::retained_manifest(self.store, self.access, area_id, file_id)
						.await?
						.ok_or_else(|| NativeError::NotFound("file unavailable".into()))?;
				let file = retained
					.into_iter()
					.find(|file| file.file_id == file_id)
					.ok_or_else(|| NativeError::NotFound("file unavailable".into()))?;
				retained_entry(self.access, area_id, &file).await?
			}
		};
		Ok(FileVersion {
			area_id,
			file_id,
			path: entry.path,
			media_type: entry.media_type,
		})
	}

	async fn preview_native(&mut self, kind: &PreviewKind) -> NativeResult<Preview> {
		let PreviewKind::File { area_id, file_id } = kind else {
			return Err(NativeError::Invalid(
				"Live previews are not available yet".into(),
			));
		};
		if let Some(existing) = preview_of_file(self.access, *file_id).await? {
			return Ok(Preview {
				id: existing.id,
				label: existing.label,
				kind: kind.clone(),
			});
		}
		for _ in 0..4 {
			let label = PreviewLabel::generate()?;
			let query = Query::insert()
				.into_table(Alias::new("core_previews"))
				.columns(
					[
						"id",
						"tenant",
						"kind",
						"area_id",
						"file_id",
						"label",
						"created_by",
					]
					.map(Alias::new),
				)
				.values(vec![
					Uuid::new_v4().into(),
					self.access.identity.tenant.clone().into(),
					"file".into(),
					(*area_id).into_value(),
					(*file_id).into_value(),
					label.as_str().to_owned().into(),
					self.access.identity.subject.clone().into(),
				])
				.map_err(NativeError::Invalid)?
				.on_conflict(
					OnConflict::column(Alias::new("file_id"))
						.do_nothing()
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder);
			crate::database::native::query(&query)
				.execute(&mut **self.access.tx)
				.await?;
			if let Some(existing) = preview_of_file(self.access, *file_id).await? {
				return Ok(Preview {
					id: existing.id,
					label: existing.label,
					kind: kind.clone(),
				});
			}
		}
		Err(NativeError::External(
			"preview label could not be allocated".into(),
		))
	}

	async fn ticket_native(&mut self, ticket: TicketIssue) -> NativeResult<()> {
		let query = Query::insert()
			.into_table(Alias::new("core_preview_tickets"))
			.columns(
				[
					"id",
					"tenant",
					"preview_id",
					"subject",
					"credential_id",
					"origin",
					"digest",
					"expires_at",
				]
				.map(Alias::new),
			)
			.values(vec![
				Uuid::new_v4().into(),
				self.access.identity.tenant.clone().into(),
				ticket.preview_id.into(),
				ticket.subject.into(),
				ticket.credential_id.into(),
				ticket.origin.into(),
				ticket.digest.into(),
				ticket.expires_at.into(),
			])
			.map_err(NativeError::Invalid)?
			.to_string(PostgresQueryBuilder);
		crate::database::native::query(&query)
			.execute(&mut **self.access.tx)
			.await?;
		Ok(())
	}
}

#[async_trait]
impl application::Scope for Scope<'_> {
	fn preview_domain(&self) -> Option<&str> {
		self.domain
	}
	fn public_port(&self) -> Option<u16> {
		self.public_port
	}
	fn origin(&self) -> &str {
		self.origin
	}
	fn subject(&self) -> &str {
		&self.access.identity.subject
	}
	fn credential_id(&self) -> Uuid {
		self.access.identity.credential_id
	}
	async fn authorize_file(
		&mut self,
		area_id: Uuid,
		file_id: Uuid,
	) -> aidash_application::Result<FileVersion> {
		self.authorize_file_native(area_id, file_id)
			.await
			.map_err(Into::into)
	}
	async fn preview(&mut self, kind: &PreviewKind) -> aidash_application::Result<Preview> {
		self.preview_native(kind).await.map_err(Into::into)
	}
	async fn ticket(&mut self, ticket: TicketIssue) -> aidash_application::Result<()> {
		self.ticket_native(ticket).await.map_err(Into::into)
	}
}

struct StoredPreview {
	id: Uuid,
	label: PreviewLabel,
}

async fn preview_of_file(access: &mut Access, file: Uuid) -> NativeResult<Option<StoredPreview>> {
	let row: Option<(Uuid, String)> = crate::database::native::query_as(
		&Query::select()
			.columns([Alias::new("id"), Alias::new("label")])
			.from(Alias::new("core_previews"))
			.and_where(Expr::col(Alias::new("kind")).eq(Expr::value("file")))
			.and_where(Expr::col(Alias::new("file_id")).eq(Expr::value(file)))
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["id", "label"])
	.fetch_optional(&mut **access.tx)
	.await?;
	match row {
		Some((id, label)) => Ok(Some(StoredPreview {
			id,
			label: PreviewLabel::parse(label)?,
		})),
		None => Ok(None),
	}
}

/// The gateway: a separate listener holding only the pool and the domain.
pub(crate) struct Gateway {
	pub store: Store,
	pub domain: String,
}

struct Resolved {
	tenant: String,
	preview_id: Uuid,
	kind: String,
	area_id: Option<Uuid>,
	file_id: Option<Uuid>,
}

impl Gateway {
	async fn resolve(&self, host: &str) -> NativeResult<Resolved> {
		let label = label_of(host, &self.domain).ok_or(NativeError::Forbidden)?;
		let label = PreviewLabel::parse(label)?;
		let row: Option<ResolvedRow> = crate::database::native::query_as(
			&Query::select()
				.columns([
					Alias::new("id"),
					Alias::new("tenant"),
					Alias::new("kind"),
					Alias::new("area_id"),
					Alias::new("file_id"),
				])
				.from(Alias::new("core_previews"))
				.and_where(Expr::col(Alias::new("label")).eq(Expr::value(label.as_str())))
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.columns(&["id", "tenant", "kind", "area_id", "file_id"])
		.fetch_optional(
			self.store
				.pool
				.in_scope(&TenancyScope::Deployment(PREVIEW_SCOPE)),
		)
		.await?;
		let (preview_id, tenant, kind, area_id, file_id) = row.ok_or(NativeError::Forbidden)?;
		Ok(Resolved {
			tenant,
			preview_id,
			kind,
			area_id,
			file_id,
		})
	}

	async fn exchange_native(&self, host: &str, ticket: &str) -> NativeResult<Exchange> {
		let preview = self.resolve(host).await?;
		let secret = PreviewSecret::parse(ticket)?;
		let mut tenant = self
			.store
			.pool
			.begin(&TenancyScope::tenant(&preview.tenant)?)
			.await?;
		let consumed: Option<(Uuid, String, Uuid, String)> = crate::database::native::query_as(
			&Query::update()
				.table(Alias::new("core_preview_tickets"))
				.value_expr(Alias::new("used_at"), Expr::cust("now()"))
				.and_where(Expr::col(Alias::new("digest")).eq(Expr::value(secret.digest())))
				.and_where(Expr::col(Alias::new("preview_id")).eq(Expr::value(preview.preview_id)))
				.and_where(Expr::col(Alias::new("used_at")).is_null())
				.and_where(Expr::cust("expires_at > now()"))
				.returning([
					Alias::new("preview_id"),
					Alias::new("subject"),
					Alias::new("credential_id"),
					Alias::new("origin"),
				])
				.to_string(PostgresQueryBuilder),
		)
		.columns(&["preview_id", "subject", "credential_id", "origin"])
		.fetch_optional(&mut *tenant)
		.await?;
		let Some((preview_id, subject, credential_id, origin)) = consumed else {
			tenant.rollback().await?;
			return Err(NativeError::Forbidden);
		};
		if preview_id != preview.preview_id {
			tenant.rollback().await?;
			return Err(NativeError::Forbidden);
		}
		let session = PreviewSecret::generate()?;
		let expires_at = Utc::now()
			+ chrono::TimeDelta::try_seconds(aidash_domain::preview::SESSION_SECONDS)
				.expect("Preview session lifetime fits");
		let insert = Query::insert()
			.into_table(Alias::new("core_preview_sessions"))
			.columns(
				[
					"id",
					"tenant",
					"preview_id",
					"subject",
					"credential_id",
					"origin",
					"digest",
					"expires_at",
				]
				.map(Alias::new),
			)
			.values(vec![
				Uuid::new_v4().into(),
				preview.tenant.clone().into(),
				preview.preview_id.into(),
				subject.into(),
				credential_id.into(),
				origin.into(),
				session.digest().into(),
				expires_at.into(),
			])
			.map_err(NativeError::Invalid)?
			.to_string(PostgresQueryBuilder);
		crate::database::native::query(&insert)
			.execute(&mut *tenant)
			.await?;
		tenant.commit().await?;
		Ok(Exchange {
			cookie: session,
			max_age: aidash_domain::preview::SESSION_SECONDS,
		})
	}

	async fn view_native(&self, host: &str, cookie: &str, path: &str) -> NativeResult<Page> {
		let preview = self.resolve(host).await?;
		if preview.kind != "file" {
			return Err(NativeError::NotFound("preview unavailable".into()));
		}
		let (Some(area_id), Some(file_id)) = (preview.area_id, preview.file_id) else {
			return Err(NativeError::NotFound("preview unavailable".into()));
		};
		let secret = PreviewSecret::parse(cookie)?;
		let session: Option<(Uuid, String, Uuid, String, DateTime<Utc>)> =
			crate::database::native::query_as(
				&Query::select()
					.columns([
						Alias::new("id"),
						Alias::new("subject"),
						Alias::new("credential_id"),
						Alias::new("origin"),
						Alias::new("expires_at"),
					])
					.from(Alias::new("core_preview_sessions"))
					.and_where(
						Expr::col(Alias::new("preview_id")).eq(Expr::value(preview.preview_id)),
					)
					.and_where(Expr::col(Alias::new("digest")).eq(Expr::value(secret.digest())))
					.and_where(Expr::cust("expires_at > CURRENT_TIMESTAMP"))
					.limit(1)
					.to_string(PostgresQueryBuilder),
			)
			.columns(&["id", "subject", "credential_id", "origin", "expires_at"])
			.fetch_optional(
				self.store
					.pool
					.in_scope(&TenancyScope::tenant(&preview.tenant)?),
			)
			.await?;
		let Some((session_id, subject, credential_id, origin, expires_at)) = session else {
			return Err(NativeError::Forbidden);
		};
		if expires_at <= Utc::now() {
			return Err(NativeError::Forbidden);
		}
		// The cookie is not authority: re-run the download decision under the
		// stored credential, so a revoked subject stops at the next request.
		let mut access = Access::begin(
			&self.store,
			&SubjectIdentity {
				http_session: None,
				credential_id,
				tenant: preview.tenant.clone(),
				subject,
			},
		)
		.await?;
		let result = async {
			let area = sessions::load(&mut access, area_id).await?;
			let current = service::files(&area)?;
			let current_root = current
				.iter()
				.find(|entry| entry.file_id == file_id)
				.cloned();
			let (files, root) = match current_root {
				Some(root) => (current, root),
				None => {
					let retained =
						retention::retained_manifest(&self.store, &mut access, area_id, file_id)
							.await?
							.ok_or_else(|| NativeError::NotFound("file unavailable".into()))?;
					let mut files = Vec::with_capacity(retained.len());
					for file in &retained {
						files.push(retained_entry(&mut access, area_id, file).await?);
					}
					let root = files
						.iter()
						.find(|entry| entry.file_id == file_id)
						.cloned()
						.ok_or_else(|| NativeError::NotFound("file unavailable".into()))?;
					(files, root)
				}
			};
			let root_kind = application::media_type(&root.media_type);
			let wanted = relative_path(path)?;
			let entry = if wanted.is_empty() {
				root.clone()
			} else if matches!(root_kind, "text/html" | "application/xhtml+xml") {
				files
					.iter()
					.find(|entry| entry.path == wanted)
					.cloned()
					.ok_or_else(|| NativeError::NotFound("file unavailable".into()))?
			} else {
				return Err(NativeError::NotFound("file unavailable".into()));
			};
			if entry.size > application::MAX_PAGE_BYTES {
				return Err(NativeError::Conflict("FILE_VERSION_TOO_LARGE".into()));
			}
			let bytes = read_all(&self.store, &mut access, &entry).await?;
			let media_type = if entry.file_id == root.file_id {
				match root_kind {
					"text/html" | "application/xhtml+xml" => "text/html; charset=utf-8".to_owned(),
					"application/pdf" => "application/pdf".to_owned(),
					_ => "application/octet-stream".to_owned(),
				}
			} else {
				entry.media_type.clone()
			};
			let content_disposition = (entry.file_id == root.file_id
				&& root_kind == "application/pdf")
				.then(|| "inline".to_owned());
			Ok(Page {
				tenant: preview.tenant.clone(),
				session: session_id,
				frame_ancestor: origin,
				media_type,
				content_disposition,
				bytes,
			})
		}
		.await;
		access.finish(result).await
	}

	async fn served_native(&self, tenant: &str, session: Uuid, bytes: u64) -> NativeResult<()> {
		if bytes == 0 {
			return Ok(());
		}
		let query = Query::update()
			.table(Alias::new("core_preview_sessions"))
			.value_expr(
				Alias::new("served_bytes"),
				Expr::cust_with_values("served_bytes + ?", [bytes as i64]),
			)
			.value_expr(Alias::new("last_seen_at"), Expr::cust("now()"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(session)))
			.to_string(PostgresQueryBuilder);
		crate::database::native::query(&query)
			.execute(self.store.pool.in_scope(&TenancyScope::tenant(tenant)?))
			.await?;
		Ok(())
	}
}

#[async_trait]
impl application::Authority for Gateway {
	async fn exchange(&self, host: &str, ticket: &str) -> aidash_application::Result<Exchange> {
		self.exchange_native(host, ticket).await.map_err(Into::into)
	}
	async fn view(&self, host: &str, cookie: &str, path: &str) -> aidash_application::Result<Page> {
		self.view_native(host, cookie, path)
			.await
			.map_err(Into::into)
	}
	async fn served(
		&self,
		tenant: &str,
		session: Uuid,
		bytes: u64,
	) -> aidash_application::Result<()> {
		self.served_native(tenant, session, bytes)
			.await
			.map_err(Into::into)
	}
}

/// The request path as a manifest-relative path; empty is the Preview's root.
fn relative_path(path: &str) -> NativeResult<String> {
	let path = path.split('?').next().unwrap_or(path);
	let decoded = percent_encoding::percent_decode_str(path)
		.decode_utf8()
		.map_err(|_| NativeError::Invalid("invalid preview path".into()))?;
	let relative = decoded.trim_start_matches('/');
	if relative.is_empty() {
		return Ok(String::new());
	}
	if relative.contains('\\')
		|| relative
			.split('/')
			.any(|part| part.is_empty() || matches!(part, "." | ".."))
	{
		return Err(NativeError::NotFound("file unavailable".into()));
	}
	Ok(relative.to_owned())
}

/// The immutable object behind a held manifest entry.
async fn retained_entry(
	access: &mut Access,
	area_id: Uuid,
	file: &RetainedFile,
) -> NativeResult<FileEntry> {
	let digest: Option<String> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("digest"))
			.from(Alias::new("core_objects"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(file.file_id)))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(&access.identity.tenant)))
			.and_where(Expr::col(Alias::new("area_id")).eq(Expr::value(area_id)))
			.and_where(Expr::col(Alias::new("kind")).is_in(["working", "superseded_working"]))
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **access.tx)
	.await?;
	Ok(FileEntry {
		file_id: file.file_id,
		path: file.path.clone(),
		digest: digest.ok_or_else(|| NativeError::NotFound("file unavailable".into()))?,
		size: file.size,
		media_type: file.media_type.clone(),
		scope: crate::apps::execution::capabilities::serializers::contracts::FileScope::Working,
		provenance: serde_json::json!({"kind": "working", "area_id": area_id}),
	})
}

async fn read_all(store: &Store, access: &mut Access, entry: &FileEntry) -> NativeResult<Vec<u8>> {
	let mut bytes = Vec::with_capacity(entry.size.min(application::MAX_PAGE_BYTES) as usize);
	while (bytes.len() as u64) < entry.size {
		let chunk = store
			.capabilities
			.read_chunk(access, entry, bytes.len() as u64)
			.await?;
		if chunk.is_empty() {
			return Err(NativeError::Conflict("OBJECT_INTEGRITY".into()));
		}
		bytes.extend_from_slice(&chunk);
	}
	Ok(bytes)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_preview_path_is_relative_to_the_manifest() {
		assert_eq!(relative_path("/").unwrap(), "");
		assert_eq!(relative_path("/index.html").unwrap(), "index.html");
		assert_eq!(
			relative_path("/assets/app.js?v=1").unwrap(),
			"assets/app.js"
		);
		assert_eq!(relative_path("/a%20b.svg").unwrap(), "a b.svg");
	}

	#[test]
	fn a_preview_path_never_escapes_the_manifest() {
		for path in ["/../secret", "/a/../b", "/a//b", "/a\\b", "/%2e%2e/secret"] {
			assert!(relative_path(path).is_err(), "{path}");
		}
		assert!(relative_path("/%ff").is_err());
	}
}
