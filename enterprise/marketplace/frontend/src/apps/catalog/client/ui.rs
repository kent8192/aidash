//! Small Reinhardt Pages components shared by the Marketplace routes.

use crate::api::{ApiError, Load, Plugin, PluginVersion, Resource, encode, price};
use reinhardt::pages::{
	component::{IntoPage, Page, PageElement},
	reactive::Signal,
};
use serde::de::DeserializeOwned;
use wasm_bindgen::JsCast;

pub fn element(tag: &'static str, class: &'static str) -> PageElement {
	PageElement::new(tag).attr("class", class)
}

pub fn text(tag: &'static str, class: &'static str, value: impl Into<String>) -> Page {
	element(tag, class)
		.child(Page::text(value.into()))
		.into_page()
}

pub fn link(href: impl Into<String>, class: &'static str, label: impl Into<String>) -> Page {
	let href = href.into();
	let mut anchor = element("a", class)
		.attr("href", href.clone())
		.child(Page::text(label.into()));
	// BFF navigation and protected downloads belong to the server, outside the
	// Reinhardt client route table.
	if href.starts_with('/') {
		anchor = anchor.attr("rel", "external");
	}
	anchor.into_page()
}

pub fn loading() -> Page {
	element("div", "loading")
		.attr("role", "status")
		.attr("aria-label", "Loading")
		.children([
			PageElement::new("span").into_page(),
			PageElement::new("span").into_page(),
			PageElement::new("span").into_page(),
			text("span", "sr-only", "Loading…"),
		])
		.into_page()
}

pub fn empty(title: &str, content: Page) -> Page {
	element("section", "empty")
		.child(text("span", "eyebrow", "A place to begin"))
		.child(text("h2", "", title))
		.child(PageElement::new("p").child(content))
		.into_page()
}

pub fn error_notice(error: ApiError, retry: Option<Box<dyn Fn()>>) -> Page {
	let mut notice = element("div", "notice error").attr("role", "alert").child(
		PageElement::new("div")
			.child(text("strong", "", "Unable to complete this request"))
			.child(text("p", "", error.message)),
	);
	if error.status == Some(401) {
		notice = notice.child(link("/auth/login", "button secondary", "Sign in again"));
	} else if let Some(retry) = retry {
		notice = notice.child(
			element("button", "secondary")
				.attr("type", "button")
				.child("Try again")
				.listener("click", move |_| retry()),
		);
	}
	notice.into_page()
}

pub fn errors(state: Signal<Option<ApiError>>) -> Page {
	Page::reactive(move || {
		state
			.get()
			.map(|error| error_notice(error, None))
			.unwrap_or(Page::Empty)
	})
}

pub fn resource_view<T: Clone + DeserializeOwned + 'static>(
	resource: Resource<T>,
	ready: impl Fn(T) -> Page + 'static,
) -> Page {
	Page::reactive(move || match resource.state.get() {
		Load::Pending => loading(),
		Load::Failed(error) => {
			let resource = resource.clone();
			error_notice(error, Some(Box::new(move || resource.reload())))
		}
		Load::Ready(value) => ready(value),
	})
}

pub fn submit_button(busy: Signal<bool>, idle: &'static str, active: &'static str) -> Page {
	let disabled = busy;
	PageElement::new("button")
		.attr("type", "submit")
		.reactive_attr("disabled", move || {
			disabled.get().then_some("disabled".into())
		})
		.child(Page::reactive(move || {
			Page::text(if busy.get() { active } else { idle })
		}))
		.into_page()
}

pub fn field(label: &'static str, control: PageElement) -> Page {
	element("label", "field")
		.child(label)
		.child(control)
		.into_page()
}

pub fn input(name: &'static str) -> PageElement {
	PageElement::new("input").attr("name", name)
}

pub fn event_value(event: &web_sys::Event) -> String {
	let Some(target) = event.target() else {
		return String::new();
	};
	if let Some(input) = target.dyn_ref::<web_sys::HtmlInputElement>() {
		input.value()
	} else if let Some(select) = target.dyn_ref::<web_sys::HtmlSelectElement>() {
		select.value()
	} else {
		String::new()
	}
}

pub fn event_form(event: &web_sys::Event) -> Option<web_sys::HtmlFormElement> {
	event.prevent_default();
	event.target()?.dyn_into().ok()
}

pub fn form_data(form: &web_sys::HtmlFormElement) -> Result<web_sys::FormData, ApiError> {
	web_sys::FormData::new_with_form(form)
		.map_err(|_| ApiError::local("Unable to read this form. Please try again."))
}

pub fn value(data: &web_sys::FormData, name: &str) -> String {
	data.get(name).as_string().unwrap_or_default()
}

pub fn permissions(values: &[String]) -> Page {
	if values.is_empty() {
		return text("p", "muted", "No permissions declared.");
	}
	element("ul", "permissions")
		.children(values.iter().map(|permission| {
			PageElement::new("li")
				.child(text("code", "", permission))
				.into_page()
		}))
		.into_page()
}

pub fn plugin_list(plugins: Vec<Plugin>) -> Page {
	element("div", "plugin-list")
		.children(plugins.iter().map(|plugin| {
			let count = plugin.declared_permissions.len();
			element("article", "plugin-row")
				.child(
					element("div", "plugin-glyph")
						.attr("aria-hidden", "true")
						.child(
							plugin
								.name
								.chars()
								.take(2)
								.collect::<String>()
								.to_uppercase(),
						),
				)
				.child(
					element("div", "plugin-summary")
						.child(text("span", "eyebrow", &plugin.publisher_name))
						.child(PageElement::new("h2").child(link(
							format!("#/plugins/{}", encode(&plugin.slug)),
							"",
							&plugin.name,
						)))
						.child(text("p", "", &plugin.description))
						.child(
							element("div", "row-meta")
								.child(text("span", "", &plugin.license))
								.child(
									plugin
										.latest_version
										.as_ref()
										.map(|version| text("code", "", format!("v{version}")))
										.unwrap_or(Page::Empty),
								)
								.child(text(
									"span",
									"",
									format!(
										"{count} declared {}",
										if count == 1 {
											"permission"
										} else {
											"permissions"
										}
									),
								)),
						),
				)
				.child(text("span", "price", price(plugin)))
				.into_page()
		}))
		.into_page()
}

pub fn versions(versions: Vec<PluginVersion>, plugin_id: &str, downloadable: bool) -> Page {
	element("div", "version-list")
		.children(versions.iter().map(|version| {
			let mut metadata = PageElement::new("dl")
				.child(text("dt", "", "SHA-256"))
				.child(PageElement::new("dd").child(text("code", "digest", &version.digest)));
			if let Some(origin) = &version.origin {
				metadata = metadata
					.child(text("dt", "", "Origin"))
					.child(text("dd", "break", origin));
			}
			let mut row = element(
				"article",
				if version.recalled {
					"version recalled"
				} else {
					"version"
				},
			)
			.child(
				element("div", "version-heading")
					.child(PageElement::new("h3").child(text("code", "", &version.version)))
					.child(text(
						"span",
						if version.recalled {
							"badge danger"
						} else {
							"badge"
						},
						if version.recalled {
							"Recalled"
						} else {
							"Available"
						},
					))
					.child(text(
						"span",
						"muted",
						format!("{} bytes", grouped(version.size_bytes)),
					)),
			)
			.child(if version.recalled {
				text(
					"p",
					"recall-reason",
					version
						.recall_reason
						.as_deref()
						.filter(|reason| !reason.is_empty())
						.unwrap_or("This version must not be used for new Runs."),
				)
			} else {
				Page::Empty
			})
			.child(metadata);
			if downloadable && !version.recalled {
				row = row.child(link(
					format!(
						"/api/marketplace/plugins/{}/versions/{}/download",
						encode(plugin_id),
						encode(&version.version)
					),
					"button secondary compact",
					"Download version",
				));
			}
			row.into_page()
		}))
		.into_page()
}

fn grouped(value: u64) -> String {
	let digits = value.to_string();
	let mut grouped = String::new();
	for (index, digit) in digits.chars().enumerate() {
		if index > 0 && (digits.len() - index).is_multiple_of(3) {
			grouped.push(',');
		}
		grouped.push(digit);
	}
	grouped
}
