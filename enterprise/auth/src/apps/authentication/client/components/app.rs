//! The Authentication Pages entry point.
use super::super::hooks::use_authentication;
pub(crate) use super::super::hooks::{Action, App, Factor, State};
use reinhardt::pages::{Page, page};

pub fn page() -> Page {
	let app = use_authentication();
	crate::view::frame(page!({
		{ crate::view::content(app.clone(), app.state.get()) }
	}))
}
