//! Agents are operating-system accounts. This crate is the part of ctx that talks to the OS
//! about them: access lists ([`acl`]) and accounts and the `ctx-agents` group ([`accounts`]).

pub mod accounts;
pub mod acl;
