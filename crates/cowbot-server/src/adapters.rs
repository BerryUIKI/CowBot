//! The adapters this build ships, built once per process.
//!
//! `Registry::new()` starts empty and each platform crate registers itself, so a
//! build with no adapters reports every stored integration as unsupported rather
//! than failing to compile. The registry is process-wide because an adapter's
//! factory is stateless — it parses configuration and returns a connection —
//! while the connections themselves are per integration.

use std::sync::OnceLock;

use cowbot_integrations::Registry;

static REGISTRY: OnceLock<Registry> = OnceLock::new();

/// The process-wide adapter registry.
///
/// Called from the route that reports whether a kind is supported and from
/// startup, so both answer the same thing.
pub fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| {
        let mut registry = Registry::new();
        register_adapters(&mut registry);
        registry
    })
}

/// Register every adapter this build compiles in.
///
/// Empty until S2: the platform crates land one at a time and add a line here.
fn register_adapters(_registry: &mut Registry) {}
