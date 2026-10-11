//! The terminal dashboard.
//!
//! [`model::Model`] owns the state and routes [`model::Msg`]s by
//! [`keys::Mode`]; it renders through ratatui and never touches the terminal
//! itself. The session watcher and cache are library code in
//! `rival_core::sessionview`; [`runtime`] wires them, the job workers and
//! the model to the real terminal.

pub mod config_check;
pub mod config_form;
pub mod config_view;
pub mod detail_view;
pub mod input;
pub mod jobs;
pub mod keys;
pub mod kill;
pub mod layout;
pub mod logview;
pub mod markdown;
pub mod model;
pub mod preview;
pub mod result_view;
pub mod runtime;
pub mod session_list;
pub mod styles;
#[cfg(test)]
pub mod testkit;
pub mod text;
pub mod viewport;
