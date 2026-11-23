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
/// The platform crates land one at a time and add a line here. A kind with no
/// line reports `supported: false` on its integration row, which is how an
/// operator learns that this build cannot run the bot they created.
fn register_adapters(registry: &mut Registry) {
    registry.register(
        cowbot_core::integration::IntegrationKind::Mattermost,
        std::sync::Arc::new(|config| {
            let config = cowbot_mattermost::MattermostConfig::from_json(config)?;
            Ok(
                std::sync::Arc::new(cowbot_mattermost::MattermostIntegration::new(config))
                    as std::sync::Arc<dyn cowbot_integrations::Integration>,
            )
        }),
    );
    registry.register(
        cowbot_core::integration::IntegrationKind::Telegram,
        std::sync::Arc::new(|config| {
            let config = cowbot_telegram::TelegramConfig::from_json(config)?;
            Ok(
                std::sync::Arc::new(cowbot_telegram::TelegramIntegration::new(config))
                    as std::sync::Arc<dyn cowbot_integrations::Integration>,
            )
        }),
    );
    registry.register(
        cowbot_core::integration::IntegrationKind::Slack,
        std::sync::Arc::new(|config| {
            let config = cowbot_slack::SlackConfig::from_json(config)?;
            Ok(
                std::sync::Arc::new(cowbot_slack::SlackIntegration::new(config))
                    as std::sync::Arc<dyn cowbot_integrations::Integration>,
            )
        }),
    );
}
