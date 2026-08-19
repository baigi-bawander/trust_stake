// Same reasoning as instructions/admin.rs: every leaf module names its
// handler function `handler`, so the blanket `module::*` re-exports below
// collide on that name. Harmless, and the blanket glob (rather than
// per-struct exports) is required so the hidden `__client_accounts_*`
// modules `#[derive(Accounts)]` generates reach the crate root, which the
// `#[program]` macro's own codegen depends on.
#![allow(ambiguous_glob_reexports)]

pub mod accept_marketplace_authority;
pub mod add_stake;
pub mod admin;
pub mod close_dispute;
pub mod expire_dispute;
pub mod grant_permit;
pub mod increase_permit;
pub mod initialize_stake;
pub mod propose_marketplace_authority;
pub mod raise_dispute;
pub mod register_marketplace;
pub mod release_permit;
pub mod release_permit_early;
pub mod resolve_dispute;
pub mod revoke_permit;
pub mod update_marketplace;
pub mod withdraw_stake;

pub use accept_marketplace_authority::*;
pub use add_stake::*;
pub use admin::*;
pub use close_dispute::*;
pub use expire_dispute::*;
pub use grant_permit::*;
pub use increase_permit::*;
pub use initialize_stake::*;
pub use propose_marketplace_authority::*;
pub use raise_dispute::*;
pub use register_marketplace::*;
pub use release_permit::*;
pub use release_permit_early::*;
pub use resolve_dispute::*;
pub use revoke_permit::*;
pub use update_marketplace::*;
pub use withdraw_stake::*;
