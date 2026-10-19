//! A local muppet (decision 7): Xmesh is its engine, `xtxp-wire` carries its
//! conversation with a host, and its application layer is a TUI or one-shot
//! commands. Hexagonal: [`core::Muppet`] knows nothing about terminals,
//! sockets or clocks; everything that touches the world plugs in through
//! [`ports`], and the binary's adapters implement them.

#[cfg(feature = "adapters")]
pub mod adapters;
pub mod core;
pub mod ops_words;
pub mod ports;
pub mod render;
