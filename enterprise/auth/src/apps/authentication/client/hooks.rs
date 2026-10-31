// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Authentication state and the lifetime of the upstream browser SDK.
mod authentication;
mod forms;
pub use authentication::{Action, App, Factor, State, use_authentication};
