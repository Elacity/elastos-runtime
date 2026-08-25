//! Per-surface logger modules for the `elastos` binary. Each surface imports its module
//! (`use crate::logger::gateway_auth as logger;`) and logs with `logger::warn!("...")` —
//! the component and call-site module path are stamped automatically.

elastos_logger::component_mod!(pub(crate) gateway_http, "gateway.http");
elastos_logger::component_mod!(pub(crate) gateway_auth, "gateway.auth");
elastos_logger::component_mod!(pub(crate) gateway_browser, "gateway.browser");
elastos_logger::component_mod!(pub(crate) gateway_home, "gateway.home");
elastos_logger::component_mod!(pub(crate) gateway_effects, "gateway.effects");
elastos_logger::component_mod!(pub(crate) gateway_content, "gateway.content");
elastos_logger::component_mod!(pub(crate) gateway_bridge, "gateway.bridge");
elastos_logger::component_mod!(pub(crate) gateway_infra, "gateway.infra");
elastos_logger::component_mod!(pub(crate) gateway_capsule, "gateway.capsule");
elastos_logger::component_mod!(pub(crate) cmd_chat, "cmd.chat");
elastos_logger::component_mod!(pub(crate) cmd_ipfs, "cmd.ipfs");
elastos_logger::component_mod!(pub(crate) cmd_run, "cmd.run");
elastos_logger::component_mod!(pub(crate) cmd_serve, "cmd.serve");
elastos_logger::component_mod!(pub(crate) cmd_share, "cmd.share");
elastos_logger::component_mod!(pub(crate) cmd_update, "cmd.update");
elastos_logger::component_mod!(pub(crate) vm_supervisor, "vm.supervisor");
elastos_logger::component_mod!(pub(crate) vm_provider, "vm.provider");
elastos_logger::component_mod!(pub(crate) host_binaries, "host.binaries");
elastos_logger::component_mod!(pub(crate) host_lock, "host.lock");
elastos_logger::component_mod!(pub(crate) host_operator, "host.operator");
elastos_logger::component_mod!(pub(crate) carrier, "carrier");
elastos_logger::component_mod!(pub(crate) collab, "collab");
