//! `smart-vault`: Agent-agnostic, ultra-fast `#![no_std]`+ASM+CSR Rust second brain
//! for Obsidian, wired into `8b-is/smart-tree`, `MemNet`, and `MemPalace`.

pub mod bm25;
pub mod capture;
pub mod cli;
pub mod contracts;
pub mod error;
pub mod hooks;
pub mod lint;
pub mod markdown;
pub mod mcp;
pub mod memnet;
pub mod mempalace;
pub mod multi_agent;
pub mod paths;
pub mod signing;
pub mod smart_tree;
pub mod transaction;
pub mod vault;

pub use error::{MemVaultError, Result};
