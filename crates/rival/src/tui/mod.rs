//! The terminal dashboard. Go: `internal/dashboard`.
//!
//! [`model::Model`] owns the state and routes [`model::Msg`]s by
//! [`keys::Mode`]; it renders through ratatui and never touches the terminal
//! itself. The session watcher and cache are library code in
//! `rival_core::sessionview`; the runtime that wires them to a real terminal
//! comes with Task 4.5.

pub mod detail_view;
pub mod input;
pub mod keys;
pub mod layout;
pub mod model;
pub mod session_list;
pub mod styles;
#[cfg(test)]
pub mod testkit;
pub mod text;
