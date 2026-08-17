use super::*;

#[path = "gateway_wallet_accounts.rs"]
mod gateway_wallet_accounts;
#[path = "gateway_wallet_app.rs"]
mod gateway_wallet_app;
#[path = "gateway_wallet_approvals.rs"]
mod gateway_wallet_approvals;
#[path = "gateway_wallet_connectors.rs"]
mod gateway_wallet_connectors;
#[path = "gateway_wallet_prices.rs"]
mod gateway_wallet_prices;
#[path = "gateway_wallet_send.rs"]
mod gateway_wallet_send;

pub(in crate::api::gateway) use gateway_wallet_accounts::*;
// `system_wallet_accounts_summary` is `pub(in crate::api)` (wider than this glob) so it can also
// reach `viewer_open::prepare_owned_grant`, a sibling of `gateway` — re-export it explicitly at
// that width; the glob above still covers everything else in the module at the narrower default.
pub(in crate::api) use gateway_wallet_accounts::system_wallet_accounts_summary;
// `runtime_wallet_data` is likewise `pub(in crate::api)` (wider than the glob above) so
// `viewer_grant_sign`, a sibling of `gateway`, can dispatch `RequestApproval`/`ListApprovals`
// directly for the dDRM delegation-sign approval flow.
pub(in crate::api) use gateway_wallet_accounts::runtime_wallet_data;
pub(in crate::api::gateway) use gateway_wallet_app::*;
pub(in crate::api::gateway) use gateway_wallet_approvals::*;
pub(crate) use gateway_wallet_connectors::ensure_wallet_connector_configured;
pub(in crate::api::gateway) use gateway_wallet_connectors::*;
pub(in crate::api::gateway) use gateway_wallet_prices::*;
pub(in crate::api::gateway) use gateway_wallet_send::*;
