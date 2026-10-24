//! Account-owned Publisher registration, listings, and immutable publication.

use crate::{
	api::{
		self, API, ApiError, Catalog, CreatedPlugin, CreatedPublisher, Load, Plugin, PluginVersion,
		PublishedVersion, Publisher, Publishers, Resource, Session, encode,
	},
	ui::*,
};
use reinhardt::pages::{
	component::{IntoPage, Page, PageElement},
	reactive::Signal,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, rc::Rc};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{JsFuture, spawn_local};

pub fn view(session: Session) -> Page {
	let publishers = Resource::<Publishers>::new(format!("{API}/publishers"));
	Page::fragment([
        element("div", "page-heading compact-heading")
            .child(PageElement::new("div")
                .child(text("span", "eyebrow accent", "For builders"))
                .child(text("h1", "", "Publisher tools"))
                .child(text("p", "", "Publish plugins under your own Publisher. Your Aidash Account owns these Publisher profiles."))).into_page(),
        resource_view(publishers, move |publishers| publisher_tools(publishers.publishers, session.clone())),
    ])
}

fn publisher_tools(initial: Vec<Publisher>, session: Session) -> Page {
	let selected = Signal::new(initial.first().cloned());
	let publishers = Signal::new(initial);
	let error = Signal::new(None);
	let busy = Signal::new(false);
	let selector = {
		Page::reactive(move || {
			let all = publishers.get();
			if all.len() <= 1 {
				return Page::Empty;
			}
			let current = selected
				.get()
				.map(|publisher| publisher.id)
				.unwrap_or_default();
			field(
				"Active Publisher",
				PageElement::new("select")
					.children(all.into_iter().map(|publisher| {
						PageElement::new("option")
							.attr("value", publisher.id.clone())
							.bool_attr("selected", publisher.id == current)
							.child(publisher.name)
							.into_page()
					}))
					.listener("change", move |event| {
						let id = event_value(&event);
						selected.set(
							publishers
								.get_untracked()
								.into_iter()
								.find(|publisher| publisher.id == id),
						);
					}),
			)
		})
	};
	let workspace = {
		let session = session.clone();
		Page::reactive(move || {
			selected
				.get()
				.map(|publisher| publisher_workspace(publisher, session.clone()))
				.unwrap_or(Page::Empty)
		})
	};
	let submit = {
		let session = session.clone();
		move |event: web_sys::Event| {
			let Some(form) = event_form(&event) else {
				return;
			};
			if busy.get_untracked() {
				return;
			}
			let data = match form_data(&form) {
				Ok(data) => data,
				Err(caught) => {
					error.set(Some(caught));
					return;
				}
			};
			busy.set(true);
			error.set(None);
			let session = session.clone();
			let name = value(&data, "name");
			spawn_local(async move {
				let response = api::write::<CreatedPublisher>(
					&format!("{API}/publishers"),
					&json!({"name": name}),
					&session,
				)
				.await;
				if busy.try_get_untracked().is_err() {
					return;
				}
				match response {
					Ok(response) => {
						let Ok(mut all) = publishers.try_get_untracked() else {
							return;
						};
						all.push(response.publisher.clone());
						let _ = publishers.try_set(all);
						let _ = selected.try_set(Some(response.publisher));
						form.reset();
					}
					Err(caught) => {
						let _ = error.try_set(Some(caught));
					}
				}
				let _ = busy.try_set(false);
			});
		}
	};
	Page::fragment([
		selector,
		workspace,
		element("form", "form-panel publisher-create")
			.listener("submit", submit)
			.child(text("h2", "", "Create a Publisher"))
			.child(text(
				"p",
				"muted",
				"Give your plugins a recognizable publishing identity.",
			))
			.child(field(
				"Publisher name",
				input("name")
					.bool_attr("required", true)
					.attr("maxlength", "160"),
			))
			.child(errors(error))
			.child(submit_button(busy, "Create Publisher", "Creating…"))
			.into_page(),
	])
}

fn publisher_workspace(publisher: Publisher, session: Session) -> Page {
	let plugins = Resource::<Catalog>::new(format!(
		"{API}/publishers/{}/plugins",
		encode(&publisher.id)
	));
	let selected = Signal::new(None::<Plugin>);
	let show_create = Signal::new(false);
	let toggle = show_create;
	let label = show_create;
	let chooser = {
		let plugins = plugins.clone();
		Page::reactive(move || match plugins.state.get() {
			Load::Pending => loading(),
			Load::Failed(error) => {
				let plugins = plugins.clone();
				error_notice(error, Some(Box::new(move || plugins.reload())))
			}
			Load::Ready(available) if available.plugins.is_empty() => {
				if show_create.get() {
					Page::Empty
				} else {
					text(
						"p",
						"muted",
						"Create your first listing, then publish an immutable version.",
					)
				}
			}
			Load::Ready(available) => {
				let current = selected.get().map(|plugin| plugin.id).unwrap_or_default();
				let plugins = plugins.clone();
				field(
					"Plugin listing",
					PageElement::new("select")
						.child(
							PageElement::new("option")
								.attr("value", "")
								.child("Choose a plugin"),
						)
						.children(available.plugins.into_iter().map(|plugin| {
							let label = format!(
								"{} · {}",
								plugin.name,
								plugin.latest_version.as_deref().unwrap_or("Draft")
							);
							PageElement::new("option")
								.attr("value", plugin.id.clone())
								.bool_attr("selected", plugin.id == current)
								.child(label)
								.into_page()
						}))
						.listener("change", move |event| {
							let id = event_value(&event);
							let plugin = match plugins.state.get_untracked() {
								Load::Ready(available) => {
									available.plugins.into_iter().find(|plugin| plugin.id == id)
								}
								_ => None,
							};
							selected.set(plugin);
						}),
				)
			}
		})
	};
	let creation = {
		let plugins = plugins.clone();
		let session = session.clone();
		let publisher_id = publisher.id.clone();
		Page::reactive(move || {
			if show_create.get() {
				create_listing(
					publisher_id.clone(),
					session.clone(),
					plugins.clone(),
					selected,
					show_create,
				)
			} else {
				Page::Empty
			}
		})
	};
	let publication = Page::reactive(move || {
		selected
			.get()
			.map(|plugin| {
				let plugins = plugins.clone();
				publish_version(plugin, session.clone(), Rc::new(move || plugins.reload()))
			})
			.unwrap_or(Page::Empty)
	});
	element("section", "publisher-workspace")
		.child(
			element("div", "section-bar")
				.child(text("h2", "", publisher.name))
				.child(
					element("button", "secondary")
						.attr("type", "button")
						.child(Page::reactive(move || {
							Page::text(if label.get() {
								"Cancel new listing"
							} else {
								"New plugin listing"
							})
						}))
						.listener("click", move |_| toggle.update(|show| *show = !*show)),
				),
		)
		.child(chooser)
		.child(creation)
		.child(publication)
		.into_page()
}

fn create_listing(
	publisher_id: String,
	session: Session,
	plugins: Resource<Catalog>,
	selected: Signal<Option<Plugin>>,
	show_create: Signal<bool>,
) -> Page {
	let error = Signal::new(None);
	let busy = Signal::new(false);
	let submit = {
		move |event: web_sys::Event| {
			let Some(form) = event_form(&event) else {
				return;
			};
			if busy.get_untracked() {
				return;
			}
			let data = match form_data(&form) {
				Ok(data) => data,
				Err(caught) => {
					error.set(Some(caught));
					return;
				}
			};
			let cents = match value(&data, "price_cents").parse::<u64>() {
				Ok(cents) if cents <= 9_007_199_254_740_991 => cents,
				_ => {
					error.set(Some(ApiError::local(
						"Enter a non-negative whole price in US cents.",
					)));
					return;
				}
			};
			let payload = json!({
				"publisher_id": publisher_id,
				"slug": value(&data, "slug"),
				"name": value(&data, "name"),
				"description": value(&data, "description"),
				"license": value(&data, "license"),
				"origin": value(&data, "origin"),
				"price_cents": cents,
				"currency": value(&data, "currency"),
				"declared_permissions": value(&data, "permissions").lines().map(str::trim).filter(|permission| !permission.is_empty()).collect::<Vec<_>>(),
			});
			busy.set(true);
			error.set(None);
			let session = session.clone();
			let plugins = plugins.clone();
			spawn_local(async move {
				let response =
					api::write::<CreatedPlugin>(&format!("{API}/plugins"), &payload, &session)
						.await;
				if busy.try_get_untracked().is_err() {
					return;
				}
				match response {
					Ok(response) => {
						let mut available = match plugins.state.try_get_untracked() {
							Ok(Load::Ready(available)) => available,
							Err(_) => return,
							_ => Catalog {
								plugins: Vec::new(),
							},
						};
						if !available
							.plugins
							.iter()
							.any(|plugin| plugin.id == response.plugin.id)
						{
							available.plugins.push(response.plugin.clone());
						}
						let _ = plugins.state.try_set(Load::Ready(available));
						let _ = selected.try_set(Some(response.plugin));
						let _ = show_create.try_set(false);
					}
					Err(caught) => {
						let _ = error.try_set(Some(caught));
					}
				}
				let _ = busy.try_set(false);
			});
		}
	};
	element("form", "form-panel")
		.listener("submit", submit)
		.child(text("h3", "", "New plugin listing"))
		.child(
			element("div", "form-grid")
				.child(field(
					"Plugin name",
					input("name")
						.bool_attr("required", true)
						.attr("maxlength", "160"),
				))
				.child(field(
					"URL slug",
					input("slug")
						.bool_attr("required", true)
						.attr("pattern", "[a-z0-9]+(-[a-z0-9]+)*")
						.attr("placeholder", "source-reader"),
				))
				.child(
					element("label", "field wide").child("Description").child(
						PageElement::new("textarea")
							.attr("name", "description")
							.bool_attr("required", true)
							.attr("rows", "3"),
					),
				)
				.child(field(
					"License",
					input("license")
						.bool_attr("required", true)
						.attr("placeholder", "Apache-2.0"),
				))
				.child(field(
					"Origin URL",
					input("origin")
						.attr("type", "url")
						.bool_attr("required", true)
						.attr("pattern", "https://.*")
						.attr("placeholder", "https://example.org/plugin"),
				))
				.child(
					element("label", "field")
						.child("Price in US cents")
						.child(
							input("price_cents")
								.attr("type", "number")
								.attr("min", "0")
								.attr("step", "1")
								.attr("value", "0")
								.bool_attr("required", true),
						)
						.child(text(
							"span",
							"hint",
							"0 for free. Paid acquisition is not available yet.",
						)),
				)
				.child(field(
					"Currency",
					input("currency")
						.attr("value", "USD")
						.bool_attr("readonly", true),
				))
				.child(
					element("label", "field wide")
						.child("Declared Permissions")
						.child(
							PageElement::new("textarea")
								.attr("name", "permissions")
								.attr("rows", "3")
								.attr("placeholder", "One permission per line"),
						),
				),
		)
		.child(errors(error))
		.child(submit_button(busy, "Create listing", "Creating…"))
		.into_page()
}

fn publish_version(plugin: Plugin, session: Session, on_published: Rc<dyn Fn()>) -> Page {
	let error = Signal::new(None);
	let result = Signal::new(None::<PluginVersion>);
	let busy = Signal::new(false);
	let submit = {
		move |event: web_sys::Event| {
			let Some(form) = event_form(&event) else {
				return;
			};
			if busy.get_untracked() {
				return;
			}
			let data = match form_data(&form) {
				Ok(data) => data,
				Err(caught) => {
					error.set(Some(caught));
					return;
				}
			};
			let version = value(&data, "version");
			let file = data.get("bundle").dyn_into::<web_sys::File>().ok();
			busy.set(true);
			error.set(None);
			result.set(None);
			let session = session.clone();
			let id = plugin.id.clone();
			let on_published = on_published.clone();
			spawn_local(async move {
				let published = async {
					let payload = publication_payload(file, version).await?;
					if busy.try_get_untracked().is_err() {
						return Ok(None);
					}
					api::write::<PublishedVersion>(
						&format!("{API}/plugins/{}/versions", encode(&id)),
						&payload,
						&session,
					)
					.await
					.map(Some)
				}
				.await;
				if busy.try_get_untracked().is_err() {
					return;
				}
				match published {
					Ok(Some(response)) => {
						if result.try_set(Some(response.version)).is_ok() {
							on_published();
						}
					}
					Ok(None) => {}
					Err(caught) => {
						let _ = error.try_set(Some(caught));
					}
				}
				let _ = busy.try_set(false);
			});
		}
	};
	let success = Page::reactive(move || {
		result
			.get()
			.map(|version| {
				element("div", "notice success")
					.attr("role", "status")
					.attr("aria-label", "Publication result")
					.child(format!("Version {} published. ", version.version))
					.child(text("code", "digest", version.digest))
					.into_page()
			})
			.unwrap_or(Page::Empty)
	});
	element("form", "form-panel").listener("submit", submit)
        .child(text("span", "eyebrow", "Publish an immutable version"))
        .child(text("h3", "", plugin.name))
        .child(text("p", "muted", "Published versions cannot be overwritten. Review the bundle and its declared permissions before publishing."))
        .child(element("div", "form-grid")
            .child(field("Version", input("version").attr("placeholder", "1.0.0").bool_attr("required", true)))
            .child(element("label", "field").child("Agent Plugin bundle")
                .child(input("bundle").attr("type", "file").attr("accept", ".json,application/json").bool_attr("required", true))
                .child(text("span", "hint", "JSON file map including root plugin.json; ASCII paths and base64 contents."))))
        .child(errors(error)).child(success).child(submit_button(busy, "Publish version", "Validating and publishing…")).into_page()
}

/// Validate the browser's transport envelope before sending it. The backend
/// owns manifest validation, decoded size, permissions, and canonical digest.
async fn publication_payload(
	file: Option<web_sys::File>,
	version: String,
) -> Result<Value, ApiError> {
	const LIMIT: usize = 12 * 1024 * 1024;
	let file = file
		.filter(|file| file.size() > 0.0)
		.ok_or_else(|| ApiError::local("Choose an Agent Plugin bundle."))?;
	if file.size() > LIMIT as f64 {
		return Err(ApiError::local("The bundle must be 12 MiB or smaller."));
	}
	let text = JsFuture::from(file.text())
		.await
		.ok()
		.and_then(|value| value.as_string())
		.ok_or_else(|| ApiError::local("Unable to read the Agent Plugin bundle."))?;
	let parsed: Value = serde_json::from_str(&text)
		.map_err(|_| ApiError::local("The bundle must contain valid JSON."))?;
	let entries = parsed
		.as_object()
		.ok_or_else(|| ApiError::local("The bundle must map file paths to base64 contents."))?;
	if entries.is_empty()
		|| entries.iter().any(|(path, content)| {
			path.is_empty()
				|| !path.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
				|| !content.is_string()
		}) {
		return Err(ApiError::local(
			"Bundle paths must be ASCII and contents must be base64 strings.",
		));
	}
	let files = entries.iter().collect::<BTreeMap<_, _>>();
	let payload = json!({ "version": version, "files": files });
	if payload.to_string().len() > LIMIT {
		return Err(ApiError::local(
			"The complete publication request must be 12 MiB or smaller.",
		));
	}
	Ok(payload)
}
