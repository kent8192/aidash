//! Account-owned Publisher registration, listings, and immutable publication.
use super::super::hooks::{Load, Resource, use_service};
use crate::apps::distribution::server_fn::*;
use crate::{
	api::{
		self, ApiError, Catalog, Plugin, PluginDraft, Publication, Publisher, Publishers, Session,
	},
	ui::*,
};
use reinhardt::pages::{Page, Signal, form, page, use_form, use_form_action, use_state};
use serde_json::Value;
use std::{collections::BTreeMap, rc::Rc};
use wasm_bindgen_futures::JsFuture;

pub fn view(session: Session) -> Page {
	let publishers: Resource<Publishers> = use_service(marketplace_publishers);
	let content = resource_view(publishers, move |publishers: Publishers| {
		publisher_tools(publishers.publishers, session.clone())
	});
	page!({
		div {
			class: "page-heading compact-heading",
			div {
				span {
					class: "eyebrow accent",
					"For builders"
				}
				h1 { "Publisher tools" }
				p { "Publish plugins under your own Publisher. Your Aidash Account owns these Publisher profiles." }
			}
		}
		{ content }
	})
}

fn publisher_tools(initial: Vec<Publisher>, session: Session) -> Page {
	let (selected, _) = use_state(initial.first().cloned());
	let (publishers, _) = use_state(initial);
	let definition = form! {
		name: PublisherForm,
		fields: {
			name: CharField {
				required,
				max_length: 160
			}
		}
	};
	// Initialization reads the form's current values. They must belong to
	// this form, not subscribe the enclosing resource/selection renderer.
	let runtime = reinhardt::pages::reactive::untracked(|| use_form(&definition).build());
	let current = session.clone();
	let create = use_form_action(&runtime, move |values| {
		let session = current.clone();
		async move { api::create_publisher(values.name, &session).await }
	})
	.on_success(move |form, response| {
		publishers.update(|all| all.push(response.publisher.clone()));
		selected.set(Some(response.publisher.clone()));
		form.reset();
	});
	page!({
		if publishers.get().len() > 1 {
			div {
				class: "field",
				label {
					for: "active-publisher",
					"Active Publisher"
				}
				select {
					id: "active-publisher",
					@change: move |event| {
						let id = event.value().unwrap_or_default();
						selected.set(
							publishers
								.get_untracked()
								.into_iter()
								.find(|publisher| publisher.id == id),
						);
					},
					for publisher in publishers.get() {
						option {
							value: publisher.id.clone(),
							selected: selected
							.get()
							.is_some_and(|current| current.id == publisher.id),
							{ publisher.name }
						}
					}
				}
			}
		}
		{ selected
		.get()
		.map(|publisher| {
			reinhardt::pages::reactive::untracked(|| {
				publisher_workspace(publisher, session.clone())
			})
		})
		.unwrap_or(Page::Empty) }
		form {
			class: "form-panel publisher-create",
			@submit: move |event| {
				event.prevent_default();
				create.submit();
			},
			h2 { "Create a Publisher" }
			p {
				class: "muted",
				"Give your plugins a recognizable publishing identity."
			}
			label {
				class: "field",
				"Publisher name" input {
					name: "name",
					required: true,
					maxlength: 160,
					bind: runtime.field(definition.name_field())
				}
			}
			{ form_error(create.error_message()) }
			{ form_submit(create.clone(), "Create Publisher", "Creating…") }
		}
	})
}

fn publisher_workspace(publisher: Publisher, session: Session) -> Page {
	let publisher_id = publisher.id.clone();
	let plugins = use_service(move || marketplace_publisher_plugins(publisher_id.clone()));
	let (selected, _) = use_state(None::<Plugin>);
	let (show_create, _) = use_state(false);
	page!({
		section {
			class: "publisher-workspace",
			div {
				class: "section-bar",
				h2 { { publisher.name } }
				button {
					type: "button",
					class: "secondary",
					@click: move |_| show_create.update(|show| *show = !*show),
					{
						if show_create.get() { "Cancel new listing" } else { "New plugin listing" }
					}
				}
			}
			{ listing_choices(plugins, selected, show_create) }
			if show_create.get() { { create_listing(
				publisher.id.clone(),
				session.clone(),
				plugins,
				selected,
				show_create,
			) } }
			{ selected
			.get()
			.map(|plugin| {
				reinhardt::pages::reactive::untracked(|| {
					publish_version(plugin, session.clone(), Rc::new(move || plugins.refetch()))
				})
			})
			.unwrap_or(Page::Empty) }
		}
	})
}

fn listing_choices(
	plugins: Resource<Catalog>,
	selected: Signal<Option<Plugin>>,
	show_create: Signal<bool>,
) -> Page {
	match plugins.get() {
		Load::Loading => loading(),
		Load::Error(error) => error_notice(error, Some(Rc::new(move || plugins.refetch()))),
		Load::Success(available) if available.plugins.is_empty() => {
			if show_create.get() {
				Page::Empty
			} else {
				page!({
					p {
						class: "muted",
						"Create your first listing, then publish an immutable version."
					}
				})
			}
		}
		Load::Success(available) => listing_selector(available, plugins, selected),
	}
}

fn listing_selector(
	available: Catalog,
	plugins: Resource<Catalog>,
	selected: Signal<Option<Plugin>>,
) -> Page {
	page!({
		div {
			class: "field",
			label {
				for: "plugin-listing",
				"Plugin listing"
			}
			select {
				id: "plugin-listing",
				@change: move |event| {
					let id = event.value().unwrap_or_default();
					let plugin = match plugins.get() {
						Load::Success(available) => {
							available.plugins.into_iter().find(|plugin| plugin.id == id)
						}
						_ => None,
					};
					selected.set(plugin);
				},
				option {
					value: "",
					"Choose a plugin"
				}
				for plugin in available.plugins {
					option {
						value: plugin.id.clone(),
						selected: selected
						.get()
						.is_some_and(|current| current.id == plugin.id),
						{ format!(
							"{} · {}",
							plugin.name,
							plugin.latest_version.as_deref().unwrap_or("Draft")
						) }
					}
				}
			}
		}
	})
}

fn create_listing(
	publisher_id: String,
	session: Session,
	plugins: Resource<Catalog>,
	selected: Signal<Option<Plugin>>,
	show_create: Signal<bool>,
) -> Page {
	// The conditional route owns the form; its initialization must not
	// subscribe that route to editable field state.
	reinhardt::pages::reactive::untracked(|| {
		let definition = form! {
			name: PluginListingForm,
			fields: {
				name: CharField {
					required,
					max_length: 160
				}
				slug: CharField { required }
				description: TextField { required }
				license: CharField { required }
				origin: UrlField { required }
				price_cents: IntegerField {
					required,
					initial: 0,
					min: 0
				}
				currency: CharField {
					initial: "USD"
				}
				permissions: TextField {}
			}
		};
		// Initialization reads the form's current values. They must belong to
		// this form, not subscribe the enclosing resource/selection renderer.
		let runtime = reinhardt::pages::reactive::untracked(|| use_form(&definition).build());
		let create = use_form_action(&runtime, move |values| {
			let publisher_id = publisher_id.clone();
			let session = session.clone();
			async move {
				let cents = u64::try_from(values.price_cents)
					.ok()
					.filter(|cents| *cents <= 9_007_199_254_740_991)
					.ok_or_else(|| {
						ApiError::local("Enter a non-negative whole price in US cents.")
					})?;
				let draft = PluginDraft {
					publisher_id,
					name: values.name,
					slug: values.slug,
					description: values.description,
					license: values.license,
					origin: values.origin,
					price_cents: cents,
					currency: values.currency,
					declared_permissions: values
						.permissions
						.lines()
						.map(str::trim)
						.filter(|permission| !permission.is_empty())
						.map(str::to_owned)
						.collect(),
				};
				api::create_plugin(draft, &session).await
			}
		})
		.on_success(move |_, response| {
			plugins.refetch();
			selected.set(Some(response.plugin.clone()));
			show_create.set(false);
		});
		page!({
			form {
				class: "form-panel",
				@submit: move |event| {
					event.prevent_default();
					create.submit();
				},
				h3 { "New plugin listing" }
				div {
					class: "form-grid",
					label {
						class: "field",
						"Plugin name" input {
							name: "name",
							required: true,
							maxlength: 160,
							bind: runtime.field(definition.name_field())
						}
					}
					label {
						class: "field",
						"URL slug" input {
							name: "slug",
							required: true,
							pattern: "[a-z0-9]+(-[a-z0-9]+)*",
							placeholder: "source-reader",
							bind: runtime.field(definition.slug_field())
						}
					}
					div {
						class: "field wide",
						label {
							for: "listing-description",
							"Description"
						}
						textarea {
							id: "listing-description",
							name: "description",
							required: true,
							rows: 3,
							bind: runtime.field(definition.description_field())
						}
					}
					label {
						class: "field",
						"License" input {
							name: "license",
							required: true,
							placeholder: "Apache-2.0",
							bind: runtime.field(definition.license_field())
						}
					}
					label {
						class: "field",
						"Origin URL" input {
							name: "origin",
							type: "url",
							required: true,
							pattern: "https://.*",
							placeholder: "https://example.org/plugin",
							bind: runtime.field(definition.origin_field())
						}
					}
					label {
						class: "field",
						"Price in US cents" input {
							name: "price_cents",
							type: "number",
							min: 0,
							step: 1,
							required: true,
							bind: runtime.field(definition.price_cents_field())
						}
						span {
							class: "hint",
							"0 for free. Paid acquisition is not available yet."
						}
					}
					label {
						class: "field",
						"Currency" input {
							name: "currency",
							readonly: true,
							bind: runtime.field(definition.currency_field())
						}
					}
					div {
						class: "field wide",
						label {
							for: "listing-permissions",
							"Declared Permissions"
						}
						textarea {
							id: "listing-permissions",
							name: "permissions",
							rows: 3,
							placeholder: "One permission per line",
							bind: runtime.field(definition.permissions_field())
						}
					}
				}
				{ form_error(create.error_message()) }
				{ form_submit(create.clone(), "Create listing", "Creating…") }
			}
		})
	})
}

fn publish_version(plugin: Plugin, session: Session, on_published: Rc<dyn Fn()>) -> Page {
	let definition = form! {
		name: PublicationForm,
		fields: {
			version: CharField { required }
			bundle: FileField { required }
		}
	};
	// Initialization reads the form's current values. They must belong to
	// this form, not subscribe the enclosing resource/selection renderer.
	let runtime = reinhardt::pages::reactive::untracked(|| use_form(&definition).build());
	let plugin_id = plugin.id.clone();
	let publish = use_form_action(&runtime, move |values| {
		let id = plugin_id.clone();
		let session = session.clone();
		async move {
			let payload = publication_payload(values.bundle, values.version).await?;
			api::publish(id, payload, &session).await
		}
	})
	.on_success(move |_, _| on_published());
	page!({
		form {
			class: "form-panel",
			@submit: move |event| {
				event.prevent_default();
				publish.submit();
			},
			span {
				class: "eyebrow",
				"Publish an immutable version"
			}
			h3 { { plugin.name } }
			p {
				class: "muted",
				"Published versions cannot be overwritten. Review the bundle and its declared permissions before publishing."
			}
			div {
				class: "form-grid",
				label {
					class: "field",
					"Version" input {
						name: "version",
						placeholder: "1.0.0",
						required: true,
						bind: runtime.field(definition.version_field())
					}
				}
				label {
					class: "field",
					"Agent Plugin bundle" input {
						name: "bundle",
						type: "file",
						accept: ".json,application/json",
						required: true,
						@change: move |event| {
							let file = event
								.files()
								.ok()
								.and_then(|files| files.first().map(|file| file.raw().clone()));
							runtime.set_value(definition.bundle_field(), file);
						}
					}
					span {
						class: "hint",
						"JSON file map including root plugin.json; ASCII paths and base64 contents."
					}
				}
			}
			{ form_error(publish.error_message()) }
			{ publish
			.result()
			.map(|result| publication_notice(result.version))
			.unwrap_or(Page::Empty) }
			{ form_submit(
				publish.clone(),
				"Publish version",
				"Validating and publishing…",
			) }
		}
	})
}
/// Validate the browser's transport envelope before sending it. The backend
/// owns manifest validation, decoded size, permissions, and canonical digest.
async fn publication_payload(
	file: Option<web_sys::File>,
	version: String,
) -> Result<Publication, ApiError> {
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
	let files = entries
		.iter()
		.map(|(path, content)| {
			(
				path.clone(),
				content.as_str().expect("checked string").to_owned(),
			)
		})
		.collect::<BTreeMap<_, _>>();
	let payload = Publication { version, files };
	if serde_json::to_vec(&payload)
		.map_err(|_| ApiError::local("Invalid publication"))?
		.len() > LIMIT
	{
		return Err(ApiError::local(
			"The complete publication request must be 12 MiB or smaller.",
		));
	}
	Ok(payload)
}
