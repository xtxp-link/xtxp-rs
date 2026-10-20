//! The XTXP muppet: the executor of the protocol (decision 7).
//!
//! A muppet is Xmesh (the CRDT engine), `xtxp-wire` (the sync state machine), and the admission
//! and hosting logic for sessions. A muppet has no views and no user; it is the engine that
//! applications drive.
//!
//! **Hexagonal architecture**: [`core::Muppet`] knows nothing about terminals, sockets, clocks or
//! rendering; everything that touches the external world plugs in through [`ports`] traits
//! ([`Transport`], [`Clock`], [`IdSource`], [`View`]).
//!
//! ## Driving the muppet: the application API
//!
//! Applications embed the muppet as a library and drive it through a simple boundary:
//! - **In**: commands sent via [`UnboundedReceiver<Command>`] (operations the application
//!   wants to make: edit, set text, set attributes).
//! - **Out**: screens received via [`View::show(&Screen)`] (the current state of the tree,
//!   which nodes changed, and the session connection state).
//! - **Entry point**: [`adapters::ws::run(mode, server, doc, view, commands)`]: one peer on
//!   y-sweet that syncs the document and conveys commands and screens.
//!
//! The same boundary works for any application: a TUI editor, a browser server that renders HTML,
//! a game or physics engine that owns a space, or anything else that needs to drive edits on a
//! y-sweet document. Applications see the document as a tree; the muppet executes operations on
//! it, syncs with the host, and tells the application what changed.
//!
//! ## One-shot commands
//!
//! For single operations (init, edit, show, replay) that don't need a live View, use the
//! headless modes in [`adapters::ws::run`]; they return an [`Outcome`] when done.

#[cfg(feature = "adapters")]
pub mod adapters;
pub mod core;
pub mod ops_words;
pub mod ports;
