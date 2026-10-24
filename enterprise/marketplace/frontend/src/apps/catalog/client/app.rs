//! Public catalog and Account-owned Library routes, using Reinhardt reactivity.

use crate::{
	api::{self, API, Catalog, Detail, Library, Load, Resource, Session, encode, price},
	publishers,
	ui::*,
};
use gloo_events::EventListener;
use reinhardt::pages::{
	component::{IntoPage, Page, PageElement},
	deps,
	reactive::Signal,
	reactive::hooks::use_retained_effect,
};
use serde_json::{Value, json};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

fn route() -> String {
	web_sys::window()
		.and_then(|window| window.location().hash().ok())
		.map(|hash| hash.trim_start_matches('#').to_owned())
		.filter(|hash| !hash.is_empty())
		.unwrap_or_else(|| "/".into())
}

pub fn view() -> Page {
	let route = Signal::new(route());
	let session = Resource::<Session>::new(format!("{API}/session"));
	let logout_error = Signal::new(None);
	let logging_out = Signal::new(false);
	if let Some(window) = web_sys::window() {
		// Browser history may remount the root route. Drop its listener with the
		// mounted Pages scope so old instances cannot retain disposed state.
		use_retained_effect(
			move || {
				let scroll_window = window.clone();
				let listener = EventListener::new(&window, "hashchange", move |_| {
					let _ = route.try_set(self::route());
					scroll_window.scroll_to_with_x_and_y(0.0, 0.0);
				});
				Some(move || drop(listener))
			},
			deps![],
		);
	}
	let account_actions = {
		let session = session.clone();
		Page::reactive(move || match session.state.get() {
			Load::Ready(current) if current.authenticated => {
				let session = session.clone();
				let error = logout_error;
				let disabled = logging_out;
				let label = logging_out;
				Page::fragment([
					link(
						"/auth/account",
						"button secondary compact",
						"Account settings",
					),
					element("button", "secondary compact")
						.attr("type", "button")
						.reactive_attr("disabled", move || {
							disabled.get().then_some("disabled".into())
						})
						.child(Page::reactive(move || {
							Page::text(if label.get() {
								"Signing out…"
							} else {
								"Sign out"
							})
						}))
						.listener("click", move |_| {
							if logging_out.get_untracked() {
								return;
							}
							logging_out.set(true);
							error.set(None);
							let session = session.clone();
							let current = current.clone();
							spawn_local(async move {
								let response =
									api::write::<Value>("/auth/logout", &json!({}), &current).await;
								if logging_out.try_get_untracked().is_err() {
									return;
								}
								match response {
									Ok(_) => {
										session.reload();
										if let Some(window) = web_sys::window() {
											let _ = window.location().set_hash("/");
										}
									}
									Err(caught) => {
										let _ = error.try_set(Some(caught));
									}
								}
								let _ = logging_out.try_set(false);
							});
						})
						.into_page(),
				])
			}
			_ => link("/auth/login", "button secondary compact", "Sign in"),
		})
	};
	let content = {
		let session = session.clone();
		Page::reactive(move || {
			let current = route.get();
			match current.as_str() {
				"/" | "" => catalog(),
				"/library" => protected(session.clone(), |_| library()),
				"/publishers" => protected(session.clone(), publishers::view),
				_ if current.starts_with("/plugins/") => {
					let slug = js_sys::decode_uri_component(&current["/plugins/".len()..])
						.ok()
						.and_then(|value| value.as_string())
						.unwrap_or_default();
					detail(slug, session.clone())
				}
				_ => empty(
					"This page could not be found",
					link("#/", "", "Return to the catalog"),
				),
			}
		})
	};
	let session_notice = {
		let session = session.clone();
		Page::reactive(move || {
			if matches!(route.get().as_str(), "/library" | "/publishers")
				|| !matches!(session.state.get(), Load::Failed(_))
			{
				return Page::Empty;
			}
			let session = session.clone();
			element("div", "notice")
				.attr("role", "status")
				.child("Sign-in status is unavailable. The public catalog is still accessible. ")
				.child(
					element("button", "text-button")
						.attr("type", "button")
						.child("Retry sign-in status")
						.listener("click", move |_| session.reload()),
				)
				.into_page()
		})
	};
	let nav = [
		("#/", "/", "Explore"),
		("#/library", "/library", "Library"),
		("#/publishers", "/publishers", "Publishers"),
	];
	Page::fragment([
		element("a", "skip-link")
			.attr("href", "#main")
			.child("Skip to content")
			.listener("click", |event| {
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
			})
			.into_page(),
		element("header", "site-header")
			.child(
				element("a", "brand")
					.attr("href", "#/")
					.attr("aria-label", "Aidash Marketplace home")
					.child(
						element("span", "brand-mark")
							.attr("aria-hidden", "true")
							.child("a"),
					)
					.child(text("strong", "", "aidash"))
					.child(element("span", "brand-divider"))
					.child(text("span", "", "marketplace")),
			)
			.child(
				PageElement::new("nav")
					.attr("aria-label", "Main navigation")
					.children(nav.map(|(href, path, label)| {
						PageElement::new("a")
							.attr("href", href)
							.reactive_attr("aria-current", move || {
								(route.get() == path).then_some("page".into())
							})
							.child(label)
							.into_page()
					})),
			)
			.child(element("div", "account-actions").child(account_actions))
			.into_page(),
		element("main", "main")
			.attr("id", "main")
			.attr("tabindex", "-1")
			.child(errors(logout_error))
			.child(session_notice)
			.child(content)
			.into_page(),
		element("footer", "site-footer")
			.child(text("span", "", "Aidash Marketplace"))
			.child(text("span", "", "Discover · Review · Make it yours"))
			.into_page(),
	])
}

fn catalog() -> Page {
	let catalog = Resource::<Catalog>::new(format!("{API}/catalog"));
	let query = Signal::new(String::new());
	let search = query;
	let count = catalog.clone();
	let results = Page::reactive(move || match catalog.state.get() {
		Load::Pending => loading(),
		Load::Failed(error) => {
			let catalog = catalog.clone();
			error_notice(error, Some(Box::new(move || catalog.reload())))
		}
		Load::Ready(catalog) => {
			let query = query.get().to_lowercase();
			if catalog.plugins.is_empty() {
				return empty(
					"The catalog is ready for its first plugin",
					Page::fragment([
						Page::text("Publish an Agent Plugin to make it discoverable here. "),
						link("#/publishers", "", "Open Publisher tools"),
						Page::text("."),
					]),
				);
			}
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
	});
	Page::fragment([
		element("div", "page-heading")
			.child(
				PageElement::new("div")
					.child(text("span", "eyebrow accent", "Agent Plugins"))
					.child(
						PageElement::new("h1")
							.child("Good tools.")
							.child(PageElement::new("br"))
							.child("More capable agents."),
					)
					.child(text(
						"p",
						"",
						"Explore plugins, review what they can access, and bring them into your own workspace.",
					)),
			)
			.child(
				element("div", "heading-aside")
					.child(element("span", "status-dot"))
					.child("Open catalog")
					.child(text("span", "", "Public metadata. Explicit permissions.")),
			)
			.into_page(),
		element("div", "section-bar")
			.child(
				PageElement::new("h2")
					.child("Explore the catalog ")
					.child(Page::reactive(move || {
						text(
							"span",
							"",
							match count.state.get() {
								Load::Ready(catalog) => catalog.plugins.len().to_string(),
								_ => "—".into(),
							},
						)
					})),
			)
			.child(
				element("label", "search")
					.child(text("span", "sr-only", "Search plugins"))
					.child(
						PageElement::new("input")
							.attr("type", "search")
							.attr("placeholder", "Search plugins or publishers")
							.listener("input", move |event| search.set(event_value(&event))),
					),
			)
			.into_page(),
		results,
	])
}

fn protected(session: Resource<Session>, ready: impl Fn(Session) -> Page + 'static) -> Page {
	resource_view(session, move |session| {
		if session.authenticated {
			ready(session)
		} else {
			empty(
				"Your Aidash Account opens the door",
				Page::fragment([
					Page::text("Sign in to manage your Library and Publishers. "),
					link("/auth/login", "", "Sign in to Marketplace"),
					Page::text("."),
				]),
			)
		}
	})
}

fn detail(slug: String, session: Resource<Session>) -> Page {
	let detail = Resource::<Detail>::new(format!("{API}/catalog/{}", encode(&slug)));
	resource_view(detail, move |detail| {
		let plugin = detail.plugin;
		let busy = Signal::new(false);
		let error = Signal::new(None);
		let controls = {
			let session = session.clone();
			let plugin = plugin.clone();
			Page::reactive(move || {
				let current = match session.state.get() {
					Load::Ready(current) if current.authenticated => current,
					_ => return link("/auth/login", "button", "Sign in to add to Library"),
				};
				if plugin.price_cents != 0 {
					return text("p", "muted", "Paid acquisition is not available yet.");
				}
				let disabled = busy;
				let label = busy;
				let id = plugin.id.clone();
				PageElement::new("button")
					.attr("type", "button")
					.reactive_attr("disabled", move || {
						disabled.get().then_some("disabled".into())
					})
					.child(Page::reactive(move || {
						Page::text(if label.get() {
							"Adding…"
						} else {
							"Add to Library"
						})
					}))
					.listener("click", move |_| {
						if busy.get_untracked() {
							return;
						}
						busy.set(true);
						error.set(None);
						let current = current.clone();
						let id = id.clone();
						spawn_local(async move {
							let response = api::write::<Value>(
								&format!("{API}/plugins/{}/acquire", encode(&id)),
								&json!({}),
								&current,
							)
							.await;
							if busy.try_get_untracked().is_err() {
								return;
							}
							match response {
								Ok(_) => {
									if let Some(window) = web_sys::window() {
										let _ = window.location().set_hash("/library");
									}
								}
								Err(caught) => {
									let _ = error.try_set(Some(caught));
								}
							}
							let _ = busy.try_set(false);
						});
					})
					.into_page()
			})
		};
		let metadata = PageElement::new("dl")
			.child(text("dt", "", "Publisher"))
			.child(text("dd", "", &plugin.publisher_name))
			.child(text("dt", "", "License"))
			.child(text("dd", "", &plugin.license))
			.child(text("dt", "", "Origin"))
			.child(text("dd", "break", &plugin.origin));
		Page::fragment([
			link("#/", "back-link", "← Explore plugins"),
			element("div", "detail-heading")
				.child(
					PageElement::new("div")
						.child(text("span", "eyebrow accent", "Agent Plugin"))
						.child(text("h1", "", &plugin.name))
						.child(text("p", "", &plugin.description)),
				)
				.child(
					element("div", "acquire")
						.child(text("span", "detail-price", price(&plugin)))
						.child(controls),
				)
				.into_page(),
			errors(error),
			element("div", "detail-grid")
				.child(
					PageElement::new("section")
						.child(text("h2", "", "Versions"))
						.child(if detail.versions.is_empty() {
							text("p", "muted", "No published versions.")
						} else {
							versions(detail.versions, &plugin.id, false)
						}),
				)
				.child(
					element("aside", "metadata")
						.child(text("h2", "", "Before you add"))
						.child(metadata)
						.child(text("h3", "", "Declared Permissions"))
						.child(permissions(&plugin.declared_permissions))
						.child(text(
							"p",
							"small muted",
							"Your Deployment reviews local approval before an Agent version can use this plugin.",
						)),
				)
				.into_page(),
		])
	})
}

fn library() -> Page {
	let library = Resource::<Library>::new(format!("{API}/library"));
	Page::fragment([
        element("div", "page-heading compact-heading")
            .child(PageElement::new("div").child(text("span", "eyebrow accent", "Your collection"))
                .child(text("h1", "", "Your Library"))
                .child(text("p", "", "Agent Plugins available to your Aidash Account. Your Deployment installs and pins its own copy.")))
            .child(link("#/", "button secondary", "Explore plugins")).into_page(),
        resource_view(library, |library| {
            if library.entries.is_empty() {
                empty("Make room for your next tool", Page::fragment([Page::text("Find a plugin in the public catalog and add it to your Library. "), link("#/", "", "Explore plugins"), Page::text(".")]))
            } else {
                element("div", "library-list").children(library.entries.into_iter().map(|entry| {
                    PageElement::new("section").child(PageElement::new("h2").child(link(format!("#/plugins/{}", encode(&entry.slug)), "", entry.name)))
                        .child(versions(entry.versions, &entry.plugin_id, true)).into_page()
                })).into_page()
            }
        }),
    ])
}
