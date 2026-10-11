//! The ctx server: owns every shell, its pseudo-terminal, its screen and scrollback, and the
//! saved record of every tab.

pub mod activity;
pub mod cold;
pub mod scan;
pub mod screen;
pub mod server;
pub mod sys;
pub mod tab;
pub mod terminal;
