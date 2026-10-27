//! Editor UI panels.
//!
//! This crate will hold the toolbar, layer panel, timeline panel, properties
//! panel and menus, built with egui for the MVP (ADR-002). Widgets only read
//! editor/document state and emit [`editor::Command`]s; they never contain
//! animation or document logic.
//!
//! Status: no panels yet. They are built in the `feat/ui-integration` branch.
