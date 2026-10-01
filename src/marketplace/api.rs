use super::{distribution, installations, storage::*, *};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::Actor},
	federation::Federation,
};
use axum::{
	Extension, Json,
	body::{Body, Bytes},
	extract::{Path, Query as Params, State},
	http::header,
	response::{IntoResponse, Response},
};
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new()
		.routes(routes!(browse, publish))
		.routes(routes!(sources))
		.routes(routes!(publication_access))
		.routes(routes!(detail))
		.routes(routes!(install))
		.routes(routes!(share))
		.routes(routes!(read_consent, consent))
		.routes(routes!(list_installations))
		.routes(routes!(installation, configure))
		.routes(routes!(compatibility, set_compatibility))
		.routes(routes!(activate))
		.routes(routes!(adopt))
		.routes(routes!(administration))
		.route_layer(axum::middleware::from_fn(boundary))
}
fn subject(actor: &Actor) -> Result<&crate::authorization::identity::SubjectIdentity> {
	match actor {
		Actor::Subject(identity) => Ok(identity),
		Actor::Operator => Err(Error::Forbidden),
	}
}
fn operator(actor: &Actor) -> Result<()> {
	if matches!(actor, Actor::Operator) {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}
/// Serialize and enqueue at most two MiB while every disclosure lease remains
/// held. Queue insertion is the bounded handoff point, ordered before revokers.
/// The response consumer cannot see that queue unless the transaction commits;
/// no database lease is tied to the client's unbounded network drain.
pub(crate) async fn handoff<T: Serialize>(
	store: &crate::store::Store,
	mut access: Access,
	result: Result<T>,
) -> Result<Response> {
	let result = async {
		let value = result?;
		let bytes = serde_json::to_vec(&value)?;
		if bytes.len() > 2_097_152 {
			return Err(Error::Invalid(
				"response exceeds two MiB; narrow the query".into(),
			));
		}
		if let Some(mut audit) = access.marketplace_audit.clone() {
			audit["outcome"] = serde_json::json!("allowed");
			store
				.event(&mut access.tx, None, "marketplace.audit", audit)
				.await?;
		}
		super::storage::credential_current(&mut access).await?;
		let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
		sender
			.try_send(Ok::<Bytes, std::convert::Infallible>(Bytes::from(bytes)))
			.map_err(|_| Error::Forbidden)?;
		drop(sender);
		let body = Body::from_stream(futures_util::stream::once(async move {
			receiver.recv().await.expect("one bounded response")
		}));
		let mut response = body.into_response();
		response.headers_mut().insert(
			header::CONTENT_TYPE,
			header::HeaderValue::from_static("application/json"),
		);
		response.headers_mut().insert(
			header::CACHE_CONTROL,
			header::HeaderValue::from_static("no-store"),
		);
		Ok(response)
	}
	.await;
	let denial = result.as_ref().err().and_then(|error| {
		access.marketplace_audit.clone().map(|mut audit| {
			audit["outcome"] = serde_json::json!(match error {
				Error::Forbidden => "denied",
				Error::Unauthorized => "unauthorized",
				Error::Conflict(_) => "conflict",
				_ => "failed",
			});
			audit
		})
	});
	let result = access.finish(result).await;
	if let Some(denial) = denial {
		let mut tx = store.pool.begin().await?;
		store
			.event(&mut tx, None, "marketplace.audit", denial)
			.await?;
		tx.commit().await?;
	}
	result
}
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
struct Browse {
	#[serde(default)]
	q: String,
	#[serde(default)]
	offset: usize,
	#[serde(default = "page_size")]
	limit: usize,
}
fn page_size() -> usize {
	50
}
#[utoipa::path(get,path="/marketplace/packages",operation_id="marketplace_browse",params(Browse),responses((status=200,body=[Summary])),security(("bearer_auth"=[])))]
async fn browse(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Params(input): Params<Browse>,
) -> Result<Response> {
	if input.limit == 0 || input.limit > 100 || input.offset > 10000 || input.q.len() > 256 {
		return Err(Error::Invalid("invalid search bounds".into()));
	}
	let mut access = begin(&f.store, subject(&actor)?, false).await?;
	operation(
		&mut access,
		"marketplace.browse",
		json!({"query":input.q,"offset":input.offset,"limit":input.limit}),
		None,
	);
	let result = async {
		let mut visible = vec![];
		let mut skipped = 0;
		let mut cursor = String::new();
		let search_query = input.q.to_lowercase();
		loop {
			let candidates: Vec<(String, Version)> =
				documents_page(&mut access.tx, "marketplace_versions", &cursor, 64).await?;
			let exhausted = candidates.len() < 64;
			for (key, candidate) in candidates {
				cursor = key;
				let result = async {
					access
						.require(
							&distribution::resource(&access, &candidate),
							"marketplace.browse",
						)
						.await?;
					distribution::distributed(&mut access, &candidate, &subject(&actor)?.tenant)
						.await?;
					distribution::summary(&mut access, &candidate, &f.store.node_id).await
				}
				.await;
				let summary = match result {
					Ok(s) => s,
					Err(Error::Forbidden) => continue,
					Err(e) => return Err(e),
				};
				// Search only public summary fields, never configuration/dependency IDs.
				let search = json!([
					summary.name,
					summary.description,
					summary.package_id,
					summary.author,
					summary.kind,
					summary.capabilities,
					summary.languages
				])
				.to_string()
				.to_lowercase();
				if !search.contains(&search_query) {
					continue;
				}
				if skipped < input.offset {
					skipped += 1;
					continue;
				}
				visible.push(summary);
				if visible.len() == input.limit {
					break;
				}
			}
			if visible.len() == input.limit || exhausted {
				break;
			}
		}
		Ok(visible)
	}
	.await;
	handoff(&f.store, access, result).await
}
#[utoipa::path(get,path="/marketplace/packages/{key}",operation_id="marketplace_detail",params(("key"=String,Path)),responses((status=200,body=Detail)),security(("bearer_auth"=[])))]
async fn detail(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(key): Path<String>,
) -> Result<Response> {
	let mut access = begin(&f.store, subject(&actor)?, false).await?;
	let result = distribution::detail(&mut access, &key, &f.store.node_id).await;
	handoff(&f.store, access, result).await
}
#[utoipa::path(post,path="/marketplace/packages",operation_id="marketplace_publish_registered",request_body=Publish,responses((status=200,body=Value)),security(("bearer_auth"=[])))]
async fn publish(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Json(input): Json<Publish>,
) -> Result<Response> {
	let mut access = begin(&f.store, subject(&actor)?, true).await?;
	let result = distribution::publish(&f.store, &mut access, &input).await;
	handoff(&f.store, access, result).await
}
#[utoipa::path(get,path="/marketplace/sources",operation_id="marketplace_sources",responses((status=200,body=[Entry])),security(("bearer_auth"=[])))]
async fn sources(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
) -> Result<Response> {
	let mut access = begin(&f.store, subject(&actor)?, false).await?;
	operation(&mut access, "marketplace.sources", json!({}), None);
	let result = async {
		let entries =
			crate::authorization::catalog::list_in(&mut access, &Default::default()).await?;
		let mut sources = vec![];
		for entry in entries {
			if matches!(entry.kind.as_str(), "agent" | "tool" | "skill")
				&& access
					.decide(
						&crate::authorization::catalog::resource(&access, &entry),
						"registry.export",
					)
					.await?
			{
				let readable = async {
					super::definitions::private_context(&mut access, &entry).await?;
					super::definitions::publication_graph(
						&mut access,
						&entry,
						&[],
						&f.store.node_id,
					)
					.await?;
					Ok(())
				}
				.await;
				match readable {
					Ok(()) => sources.push(entry),
					Err(Error::Forbidden) => {}
					Err(e) => return Err(e),
				}
			}
		}
		Ok(sources)
	}
	.await;
	handoff(&f.store, access, result).await
}
#[utoipa::path(post,path="/marketplace/packages/{key}/install",operation_id="marketplace_install_scoped",params(("key"=String,Path)),request_body=Install,responses((status=200,body=InstallationRevision)),security(("bearer_auth"=[])))]
async fn install(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(key): Path<String>,
	Json(input): Json<Install>,
) -> Result<Response> {
	let mut access = begin(&f.store, subject(&actor)?, true).await?;
	let result = installations::install(&f.store, &mut access, &key, &input).await;
	handoff(&f.store, access, result).await
}
#[utoipa::path(put,path="/marketplace/packages/{key}/audience",operation_id="marketplace_share",params(("key"=String,Path)),request_body=AudienceInput,responses((status=200,body=Audience)),security(("bearer_auth"=[])))]
async fn share(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(key): Path<String>,
	Json(input): Json<AudienceInput>,
) -> Result<Response> {
	let mut access = begin(&f.store, subject(&actor)?, true).await?;
	let result = distribution::share(&f.store, &mut access, &key, None, input).await;
	handoff(&f.store, access, result).await
}
#[utoipa::path(put,path="/marketplace/packages/{key}/consents/{tenant}",operation_id="marketplace_consent",params(("key"=String,Path),("tenant"=String,Path)),request_body=AudienceInput,responses((status=200,body=Audience)),security(("bearer_auth"=[])))]
async fn consent(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((key, tenant)): Path<(String, String)>,
	Json(input): Json<AudienceInput>,
) -> Result<Response> {
	let mut access = begin(&f.store, subject(&actor)?, true).await?;
	let result = distribution::share(&f.store, &mut access, &key, Some(&tenant), input).await;
	handoff(&f.store, access, result).await
}
#[utoipa::path(get,path="/marketplace/packages/{key}/consents/{tenant}",operation_id="marketplace_read_consent",params(("key"=String,Path),("tenant"=String,Path)),responses((status=200,body=Audience)),security(("bearer_auth"=[])))]
async fn read_consent(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((key, tenant)): Path<(String, String)>,
) -> Result<Response> {
	crate::authorization::policy::identifier(&tenant)?;
	let mut access = begin(&f.store, subject(&actor)?, false).await?;
	operation(
		&mut access,
		"marketplace.redistribution.manage",
		json!({"key":key,"redistributor":tenant}),
		None,
	);
	let result = async {
		let version: Version = get(&mut access.tx, "marketplace_versions", &key)
			.await?
			.ok_or(Error::Forbidden)?;
		if version.owner_tenant != access.identity.tenant {
			return Err(Error::Forbidden);
		}
		access
			.require(
				&distribution::resource(&access, &version),
				"marketplace.redistribution.manage",
			)
			.await?;
		let consent_key = super::storage::key(&(&key, &tenant));
		let current: Audience = get(&mut access.tx, "marketplace_consents", &consent_key)
			.await?
			.unwrap_or(Audience {
				revision: 0,
				tenants: Default::default(),
			});
		authority(
			&mut access,
			format!("marketplace_consents:{consent_key}"),
			current.revision,
		);
		Ok(current)
	}
	.await;
	handoff(&f.store, access, result).await
}
#[utoipa::path(get,path="/marketplace/installations",operation_id="marketplace_installations",responses((status=200,body=[InstallationRevision])),security(("bearer_auth"=[])))]
async fn list_installations(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
) -> Result<Response> {
	let mut access = begin(&f.store, subject(&actor)?, false).await?;
	let result = async {
		use sea_orm::sea_query::{Alias, Expr, Order, PostgresQueryBuilder, Query};
		let documents: Vec<serde_json::Value> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("document"))
				.from(Alias::new("marketplace_installations"))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
				.order_by(Alias::new("key"), Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.bind(&access.identity.tenant)
		.fetch_all(&mut **access.tx)
		.await?;
		let mut visible = vec![];
		for document in documents {
			let install: Installation = serde_json::from_value(document)?;
			match installations::view(&mut access, &install.id, None, &f.store.node_id).await {
				Ok(view) => visible.push(view),
				Err(Error::Forbidden) => {}
				Err(e) => return Err(e),
			}
		}
		Ok(visible)
	}
	.await;
	handoff(&f.store, access, result).await
}
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
struct RevisionQuery {
	revision: Option<i64>,
}
#[utoipa::path(get,path="/marketplace/installations/{id}",operation_id="marketplace_installation",params(("id"=String,Path),RevisionQuery),responses((status=200,body=InstallationRevision)),security(("bearer_auth"=[])))]
async fn installation(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<String>,
	Params(query): Params<RevisionQuery>,
) -> Result<Response> {
	let mut access = begin(&f.store, subject(&actor)?, false).await?;
	let result = installations::view(&mut access, &id, query.revision, &f.store.node_id).await;
	handoff(&f.store, access, result).await
}
#[utoipa::path(post,path="/marketplace/installations/{id}",operation_id="marketplace_configure",params(("id"=String,Path)),request_body=Configure,responses((status=200,body=InstallationRevision)),security(("bearer_auth"=[])))]
async fn configure(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<String>,
	Json(input): Json<Configure>,
) -> Result<Response> {
	let mut access = begin(&f.store, subject(&actor)?, true).await?;
	let result = installations::configure(&f.store, &mut access, &id, &input).await;
	handoff(&f.store, access, result).await
}
#[utoipa::path(get,path="/marketplace/compatibility",operation_id="marketplace_compatibility",responses((status=200,body=Compatibility)),security(("bearer_auth"=[])))]
async fn compatibility(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
) -> Result<Json<Compatibility>> {
	operator(&actor)?;
	let mut tx = f.store.pool.begin().await?;
	Ok(Json(
		get(&mut tx, "marketplace_gate", "v1")
			.await?
			.ok_or(Error::Forbidden)?,
	))
}
#[utoipa::path(put,path="/marketplace/compatibility",operation_id="marketplace_set_compatibility",request_body=CompatibilityInput,responses((status=200,body=Compatibility)),security(("bearer_auth"=[])))]
async fn set_compatibility(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	browser: Option<Extension<crate::dashboard_auth::BrowserOrigin>>,
	Json(input): Json<CompatibilityInput>,
) -> Result<Json<Compatibility>> {
	operator(&actor)?;
	if input.enabled && !input.compatible_instances_confirmed {
		return Err(Error::Invalid(
			"confirm every serving instance and worker supports Marketplace contract 1".into(),
		));
	}
	let origin = browser.as_ref().map(|Extension(origin)| origin);
	let mut tx = operator_begin(&f.store, origin).await?;
	lock(&mut tx, true).await?;
	let current: Compatibility = get(&mut tx, "marketplace_gate", "v1")
		.await?
		.ok_or(Error::Forbidden)?;
	if input.expected_revision != current.revision {
		return Err(conflict());
	}
	let next = Compatibility {
		enabled: input.enabled,
		revision: current.revision + 1,
		contract: 1,
	};
	put(&mut tx, "marketplace_gate", "v1", &next).await?;
	f.store
		.event(
			&mut tx,
			None,
			"marketplace.compatibility_changed",
			json!(next),
		)
		.await?;
	operator_commit(tx, origin).await?;
	Ok(Json(next))
}
#[utoipa::path(post,path="/marketplace/installations/{id}/activation",operation_id="marketplace_activate",params(("id"=String,Path)),request_body=Activate,responses((status=200,body=Installation)),security(("bearer_auth"=[])))]
async fn activate(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	browser: Option<Extension<crate::dashboard_auth::BrowserOrigin>>,
	Path(id): Path<String>,
	Json(input): Json<Activate>,
) -> Result<Json<Installation>> {
	operator(&actor)?;
	Ok(Json(
		installations::activate(
			&f.store,
			&id,
			input,
			browser.as_ref().map(|Extension(origin)| origin),
		)
		.await?,
	))
}
#[utoipa::path(post,path="/marketplace/adoptions",operation_id="marketplace_adopt",request_body=Adopt,responses((status=200,body=Installation)),security(("bearer_auth"=[])))]
async fn adopt(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	browser: Option<Extension<crate::dashboard_auth::BrowserOrigin>>,
	Json(input): Json<Adopt>,
) -> Result<Json<Installation>> {
	operator(&actor)?;
	Ok(Json(
		installations::adopt(
			&f.store,
			input,
			browser.as_ref().map(|Extension(origin)| origin),
		)
		.await?,
	))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
struct AdministrationQuery {
	tenant: String,
}
#[utoipa::path(get,path="/marketplace/administration",operation_id="marketplace_administration",params(AdministrationQuery),responses((status=200,body=[InstallationRevision])),security(("bearer_auth"=[])))]
async fn administration(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Params(query): Params<AdministrationQuery>,
) -> Result<Json<Vec<InstallationRevision>>> {
	operator(&actor)?;
	crate::authorization::policy::identifier(&query.tenant)?;
	let mut tx = f.store.pool.begin().await?;
	lock(&mut tx, false).await?;
	let installs: Vec<Installation> = documents(&mut tx, "marketplace_installations").await?;
	let revisions: Vec<Revision> = documents(&mut tx, "marketplace_revisions").await?;
	let mut result = vec![];
	for rev in revisions.into_iter().filter(|r| r.tenant == query.tenant) {
		let install = installs
			.iter()
			.find(|i| i.id == rev.installation)
			.ok_or(Error::Forbidden)?
			.clone();
		let approved = installations::approved(
			&mut tx,
			&query.tenant,
			&super::definitions::reference(&rev.entry),
		)
		.await?;
		result.push(InstallationRevision {
			installation: install,
			revision: rev.revision,
			entry: rev.entry,
			digest: rev.digest,
			config: rev.config,
			dependencies: rev.dependencies,
			bindings: rev.bindings,
			approved,
			actions: vec![],
		});
	}
	tx.commit().await?;
	Ok(Json(result))
}

async fn boundary(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
	let mut response =
		match tokio::time::timeout(std::time::Duration::from_secs(30), next.run(request)).await {
			Ok(response) => response,
			Err(_) => Error::IdentityStatusUnavailable.into_response(),
		};
	response.headers_mut().insert(
		header::CACHE_CONTROL,
		header::HeaderValue::from_static("no-store"),
	);
	response
}

#[utoipa::path(post,path="/marketplace/publication-access",operation_id="marketplace_publication_access",request_body=Publish,responses((status=200,body=PublicationPreview)),security(("bearer_auth"=[])))]
async fn publication_access(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Json(input): Json<Publish>,
) -> Result<Response> {
	let mut access = begin(&f.store, subject(&actor)?, false).await?;
	operation(
		&mut access,
		"marketplace.publication_preview",
		json!({"source":input.source,"package_id":input.package_id.chars().take(256).collect::<String>()}),
		None,
	);
	let result = match distribution::prepare(&f.store, &mut access, &input).await {
		Ok(version) => Ok(PublicationPreview {
			allowed: true,
			version: Some(version.version),
		}),
		Err(Error::Forbidden) => Ok(PublicationPreview {
			allowed: false,
			version: None,
		}),
		Err(e) => Err(e),
	};
	handoff(&f.store, access, result).await
}
