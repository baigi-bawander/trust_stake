// Every leaf module below names its handler function `handler`, so
// `module::*` from more than one of them re-exports that name
// ambiguously. Harmless: `lib.rs` always reaches `handler` through the
// full module path (`instructions::admin::initialize_config::handler`),
// never the ambiguous flattened name. The blanket glob (rather than
// narrowing to just the account constraints struct) is still required,
// because `#[derive(Accounts)]` also generates a hidden
// `__client_accounts_*` module per struct that the `#[program]` macro's
// own codegen needs re-exported up to the crate root.
#![allow(ambiguous_glob_reexports)]

pub mod accept_config_authority;
pub mod initialize_config;
pub mod propose_config_authority;

pub use accept_config_authority::*;
pub use initialize_config::*;
pub use propose_config_authority::*;
