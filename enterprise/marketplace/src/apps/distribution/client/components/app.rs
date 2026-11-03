// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Public catalog and Account-owned Library routes.
use super::super::hooks::{Load, Resource, use_hash_route, use_service};
use crate::apps::distribution::{serializers::Acquisition, server_fn::*};
use crate::{
	api::{self, ApiError, Catalog, Detail, Library, Session, encode, price},
	publishers,
	ui::*,
};
use reinhardt::pages::{Action, Page, page, use_action, use_state};
use wasm_bindgen::JsCast;

pub fn view() -> Page {
	let route = use_hash_route();
	let session = use_service(marketplace_session);
	let logout = use_action(move |current: Session| async move { api::logout(&current).await })
		.on_success(move |_| {
			session.refetch();
			if let Some(window) = web_sys::window() {
				let _ = window.location().set_hash("/");
			}
		});
	let nav = [
		("#/", "/", "Explore"),
		("#/library", "/library", "Library"),
		("#/publishers", "/publishers", "Publishers"),
	];
	page!({
		a {
			class: "skip-link",
			href: "#main",
			@click: move |event| {
				event.prevent_default();
				if let Some(main) = web_sys::window()
					.and_then(|window| window.document())
					.and_then(|document| document.get_element_by_id("main"))
				{
					if let Some(main) = main.dyn_ref::<web_sys::HtmlElement>() {
						let _ = main.focus();
					}
					main.scroll_into_view();
				}
			},
			"Skip to content"
		}
		header {
			class: "site-header",
			a {
				class: "brand",
				href: "#/",
				aria_label: "Aidash Marketplace home",
				span {
					class: "brand-mark",
					aria_hidden: "true",
					"a"
				}
				strong { "aidash" }
				span {
					class: "brand-divider"
				}
				span { "marketplace" }
			}
			nav {
				aria_label: "Main navigation",
				for (href, path, label) in nav {
					a {
						href: href,
						aria_current: if route.get() == path { "page" } else { "false" },
						{ label }
					}
				}
			}
			div {
				class: "account-actions",
				{ account_controls(session, logout) }
			}
		}
		main {
			class: "main",
			id: "main",
			tabindex: -1,
			{ logout
			.error()
			.map(|error| error_notice(error, None))
			.unwrap_or(Page::Empty) }
			if !matches!(route.get().as_str(), "/library" | "/publishers") && matches!(session.get(), Load::Error(_)) {
				div {
					class: "notice",
					role: "status",
					"Sign-in status is unavailable. The public catalog is still accessible. " button {
						type: "button",
						class: "text-button",
						@click: move |_| session.refetch(),
						"Retry sign-in status"
					}
				}
			} {
				match route.get().as_str() {
					"/"|"" => catalog(),
					"/library" => protected(session, self::library_for_session),
					"/publishers" => protected(session, publishers::view),
					current if current.starts_with("/plugins/") => {
						let slug = js_sys::decode_uri_component(&current["/plugins/".len()..]).ok().and_then(|value| value.as_string()).unwrap_or_default();
						detail(slug, session)
					}_ => empty("This page could not be found", link("#/", "", "Return to the catalog")),
				}
			}
		}
		footer {
			class: "site-footer",
			span { "Aidash Marketplace" }
			span { "Discover · Review · Make it yours" }
		}
	})
}

fn account_controls(session: Resource<Session>, logout: Action<Session, ApiError>) -> Page {
	match session.get() {
		Load::Success(current) if current.authenticated => page!({
			{
				link(
					"/auth/account",
					"button secondary compact",
					"Account settings",
				)
			}
			{
				action_button(
					logout,
					current.clone(),
					"secondary compact",
					"Sign out",
					"Signing out…",
				)
			}
		}),
		_ => link("/auth/login", "button secondary compact", "Sign in"),
	}
}

fn catalog() -> Page {
	let catalog: Resource<Catalog> = use_service(marketplace_catalog);
	let (query, _) = use_state(String::new());
	page!({
		div {
			class: "page-heading",
			div {
				span {
					class: "eyebrow accent",
					"Agent Plugins"
				}
				h1 {
					"Good tools." br {}"More capable agents."
				}
				p { "Explore plugins, review what they can access, and bring them into your own workspace." }
			}
			div {
				class: "heading-aside",
				span {
					class: "status-dot"
				}"Open catalog" span { "Public metadata. Explicit permissions." }
			}
		}
		div {
			class: "section-bar",
			h2 {
				"Explore the catalog " span { {
					match catalog.get() {
						Load::Success(catalog) => catalog.plugins.len().to_string(),
						_ =>"—".into()
					}
				} }
			}
			label {
				class: "search",
				span {
					class: "sr-only",
					"Search plugins"
				}
				input {
					type: "search",
					placeholder: "Search plugins or publishers",
					bind: query
				}
			}
		} {
			match catalog.get() {
				Load::Loading => loading(),
				Load::Error(error) => error_notice(error, Some(std::rc::Rc::new(move || catalog.refetch()))),
				Load::Success(catalog) => catalog_content(catalog, query.get()),
			}
		}
	})
}

fn catalog_content(catalog: Catalog, query: String) -> Page {
	if catalog.plugins.is_empty() {
		return empty(
			"The catalog is ready for its first plugin",
			page!({
				"Publish an Agent Plugin to make it discoverable here. " { link("#/publishers", "", "Open Publisher tools") }"."
			}),
		);
	}
	let query = query.to_lowercase();
	let plugins = catalog
		.plugins
		.into_iter()
		.filter(|plugin| {
			format!(
				"{} {} {}",
				plugin.name, plugin.description, plugin.publisher_name
			)
			.to_lowercase()
			.contains(&query)
		})
		.collect::<Vec<_>>();
	if plugins.is_empty() {
		empty(
			"No matching plugins",
			Page::text("Try another name, publisher, or keyword."),
		)
	} else {
		plugin_list(plugins)
	}
}

fn protected(session: Resource<Session>, ready: fn(Session) -> Page) -> Page {
	resource_view(session, move |session| {
		if session.authenticated {
			ready(session)
		} else {
			empty(
				"Your Aidash Account opens the door",
				page!({
					"Sign in to manage your Library and Publishers. " { link("/auth/login", "", "Sign in to Marketplace") }"."
				}),
			)
		}
	})
}

fn detail(slug: String, session: Resource<Session>) -> Page {
	let detail = use_service(move || marketplace_detail(slug.clone()));
	resource_view(detail, move |detail| detail_content(detail, session))
}

fn detail_content(detail: Detail, session: Resource<Session>) -> Page {
	let plugin = detail.plugin.clone();
	let id = plugin.id.clone();
	let acquire = use_action(move |current: Session| {
		let id = id.clone();
		async move { api::acquire(id, &current).await }
	})
	.on_success(move |_| {
		if let Some(window) = web_sys::window() {
			let _ = window.location().set_hash("/library");
		}
	});
	page!({
		{ link("#/", "back-link", "← Explore plugins") }
		div {
			class: "detail-heading",
			div {
				span {
					class: "eyebrow accent",
					"Agent Plugin"
				}
				h1 { { plugin.name } }
				p { { plugin.description } }
			}
			div {
				class: "acquire",
				span {
					class: "detail-price",
					{ price(&plugin) }
				}
				{ acquire_controls(session, plugin.price_cents, acquire) }
			}
		}
		{ acquire
		.error()
		.map(|error| error_notice(error, None))
		.unwrap_or(Page::Empty) }
		div {
			class: "detail-grid",
			section {
				h2 { "Versions" }
				if detail.versions.is_empty() {
					p {
						class: "muted",
						"No published versions."
					}
				} else { { versions(detail.versions.clone(), &plugin.id, false) } }
			}
			aside {
				class: "metadata",
				h2 { "Before you add" }
				dl {
					dt { "Publisher" }
					dd { { plugin.publisher_name } }
					dt { "License" }
					dd { { plugin.license } }
					dt { "Origin" }
					dd {
						class: "break",
						{ plugin.origin }
					}
				}
				h3 { "Declared Permissions" }
				{ permissions(&plugin.declared_permissions) }
				p {
					class: "small muted",
					"Your Deployment reviews local approval before an Agent version can use this plugin."
				}
			}
		}
	})
}

fn acquire_controls(
	session: Resource<Session>,
	price_cents: u64,
	acquire: Action<Acquisition, ApiError>,
) -> Page {
	match session.get() {
		Load::Success(current) if current.authenticated => {
			if price_cents != 0 {
				page!({
					p {
						class: "muted",
						"Paid acquisition is not available yet."
					}
				})
			} else {
				action_button(acquire, current, "", "Add to Library", "Adding…")
			}
		}
		_ => link("/auth/login", "button", "Sign in to add to Library"),
	}
}

fn library() -> Page {
	let library: Resource<Library> = use_service(marketplace_library);
	let content = resource_view(library, library_content);
	page!({
		div {
			class: "page-heading compact-heading",
			div {
				span {
					class: "eyebrow accent",
					"Your collection"
				}
				h1 { "Your Library" }
				p { "Agent Plugins available to your Aidash Account. Your Deployment installs and pins its own copy." }
			}
			{ link("#/", "button secondary", "Explore plugins") }
		}
		{ content }
	})
}

fn library_content(library: Library) -> Page {
	if library.entries.is_empty() {
		empty(
			"Make room for your next tool",
			page!({
				"Find a plugin in the public catalog and add it to your Library. " { link("#/", "", "Explore plugins") }"."
			}),
		)
	} else {
		let entries = library
			.entries
			.into_iter()
			.map(|entry| {
				page!({
					section {
						h2 { { link(
							format!("#/plugins/{}", encode(&entry.slug)),
							"",
							entry.name.clone(),
						) } }
						{ versions(entry.versions.clone(), &entry.plugin_id, true) }
					}
				})
			})
			.collect::<Vec<_>>();
		page!({
			div {
				class: "library-list",
				{ Page::fragment(entries.clone()) }
			}
		})
	}
}

fn library_for_session(_: Session) -> Page {
	library()
}
