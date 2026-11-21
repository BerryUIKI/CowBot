//! Mapping a platform account to a CowBot user (ADR-0056).
//!
//! `Sender::User(String)` carries a CowBot `UserId`, and the server hard-codes
//! the authenticated user when a browser posts. An adapter has the opposite
//! problem: it knows who wrote the message on the platform and needs a CowBot
//! user to attribute it to.
//!
//! The alternative considered was a `Sender::External` variant. It is honest, and
//! it changes every match on `Sender` — the service's three, the desktop's
//! renderer, the generated bindings — and still leaves the approval audit with no
//! name to show. Provisioning a real user row keeps every existing path working
//! unchanged, so that is what this does.

use cowbot_core::error::CowResult;
use cowbot_core::identity::{Membership, Role, User, UserId};
use cowbot_core::integration::{IntegrationKind, IntegrationPrincipal, IntegrationRecord};
use cowbot_store::Store;
use tracing::debug;

use crate::model::ExternalAuthor;

/// The CowBot user a platform account speaks as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub user_id: UserId,
    /// The name to attribute the message to.
    pub display_name: String,
    /// Whether this call created the row.
    ///
    /// Reported so a supervisor can count provisioned accounts and an operator
    /// can be told what a first message did, rather than only seeing the effect.
    pub provisioned: bool,
}

/// Resolve `author` to a CowBot user, creating one on first sight.
///
/// The created row has:
///
/// * an id derived from the integration and the account, so two messages
///   arriving at once compute the same id and the second insert is a no-op
///   rather than a duplicate account;
/// * no password hash, so the row can never sign in — an empty hash does not
///   verify against any password;
/// * a `Member` membership, because a message in a channel the workspace owns is
///   the workspace's work, and refusing to store it would lose the question.
pub async fn resolve_principal(
    store: &Store,
    integration: &IntegrationRecord,
    author: &ExternalAuthor,
) -> CowResult<Principal> {
    let existing = store
        .integration_principal(&integration.id, &author.external_id)
        .await?;

    match existing {
        Some(mapped) => Ok(refresh_name(store, &mapped, author).await?),
        None => provision(store, integration, author).await,
    }
}

/// Keep a mapped account's name current.
///
/// A platform lets an account rename itself, and the name is what the timeline
/// shows and what an agent prompt renders. Updating it here rather than on every
/// message means the write only happens when the name actually changed.
async fn refresh_name(
    store: &Store,
    mapped: &IntegrationPrincipal,
    author: &ExternalAuthor,
) -> CowResult<Principal> {
    let name = author.display_name.trim();
    if name.is_empty() || name == mapped.display_name {
        return Ok(Principal {
            user_id: mapped.user_id.clone(),
            display_name: mapped.display_name.clone(),
            provisioned: false,
        });
    }

    store
        .save_integration_principal(&IntegrationPrincipal {
            display_name: name.to_string(),
            ..mapped.clone()
        })
        .await?;
    if let Some(user) = store.get_user(&mapped.user_id).await? {
        store.update_user(&user.id, name, &user.email).await?;
    }

    Ok(Principal {
        user_id: mapped.user_id.clone(),
        display_name: name.to_string(),
        provisioned: false,
    })
}

/// Create the CowBot user for a platform account.
async fn provision(
    store: &Store,
    integration: &IntegrationRecord,
    author: &ExternalAuthor,
) -> CowResult<Principal> {
    let kind: IntegrationKind = integration.kind;
    let user_id = kind.bridge_user_id(&integration.id, &author.external_id);
    let email = kind.bridge_email(&integration.id, &author.external_id);
    let display_name = display_name_for(kind, author);

    // The id is derived, so a user row can outlive a lost mapping row. Reusing it
    // is what keeps the id stable across that, and it is why this is a check
    // rather than an unconditional insert.
    let provisioned = store.get_user(&user_id).await?.is_none();
    if provisioned {
        // The derived id is the point: `User::new` would generate its own, and
        // the membership below would then reference a row that was never
        // inserted. Building the value here is what keeps the two in step.
        store
            .create_user(&User {
                id: user_id.clone(),
                name: display_name.clone(),
                email,
                password_hash: String::new(),
                created_at: chrono::Utc::now(),
            })
            .await?;
        store
            .add_member(&Membership::new(
                user_id.clone(),
                integration.workspace_id.clone(),
                Role::Member,
            ))
            .await?;
    }

    store
        .save_integration_principal(&IntegrationPrincipal {
            integration_id: integration.id.clone(),
            external_id: author.external_id.clone(),
            user_id: user_id.clone(),
            display_name: display_name.clone(),
        })
        .await?;

    if provisioned {
        debug!(
            integration = %integration.id,
            kind = integration.kind.as_str(),
            external = %author.external_id,
            "Provisioned a CowBot user for an IM account"
        );
    }

    Ok(Principal {
        user_id,
        display_name,
        provisioned,
    })
}

/// The name to show for a platform account.
///
/// The platform's own name when it gave one, and a readable derivation otherwise
/// — an empty name would render as a blank author in the timeline.
fn display_name_for(kind: IntegrationKind, author: &ExternalAuthor) -> String {
    let name = author.display_name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    format!("{} {}", kind_label(kind), short_id(&author.external_id))
}

/// A platform's name, capitalised for a display name.
fn kind_label(kind: IntegrationKind) -> &'static str {
    match kind {
        IntegrationKind::Mattermost => "Mattermost",
        IntegrationKind::Telegram => "Telegram",
        IntegrationKind::Slack => "Slack",
        IntegrationKind::Feishu => "Feishu",
    }
}

/// The last four characters of an external id, so two unnamed accounts from one
/// platform are still distinguishable.
fn short_id(external_id: &str) -> String {
    let characters: Vec<char> = external_id.chars().collect();
    let start = characters.len().saturating_sub(4);
    characters[start..].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowbot_core::identity::WorkspaceId;

    /// Insert an integration and read it back, so its row really exists.
    ///
    /// A hand-built record has an id the database never saw, and the principal
    /// mapping's foreign key would then refuse the write — the bug this helper
    /// exists to keep out of the tests.
    async fn create_integration(
        store: &Store,
        workspace_id: &WorkspaceId,
        kind: IntegrationKind,
    ) -> IntegrationRecord {
        let new =
            cowbot_core::integration::NewIntegration::new(workspace_id, kind, "Support", "{}");
        store.create_integration(&new).await.unwrap();
        store.get_integration(&new.id).await.unwrap().unwrap()
    }

    #[test]
    fn an_unnamed_account_still_gets_a_readable_name() {
        let name = display_name_for(
            IntegrationKind::Telegram,
            &ExternalAuthor {
                external_id: "88213".into(),
                display_name: "   ".into(),
                is_bot: false,
            },
        );
        assert_eq!(name, "Telegram 8213");
    }

    #[test]
    fn a_named_account_keeps_the_name_the_platform_gave_it() {
        let name = display_name_for(
            IntegrationKind::Slack,
            &ExternalAuthor {
                external_id: "U024".into(),
                display_name: "Ada Lovelace".into(),
                is_bot: false,
            },
        );
        assert_eq!(name, "Ada Lovelace");
    }

    #[test]
    fn an_unnamed_account_shorter_than_four_characters_does_not_panic() {
        let name = display_name_for(
            IntegrationKind::Feishu,
            &ExternalAuthor {
                external_id: "ou".into(),
                display_name: String::new(),
                is_bot: false,
            },
        );
        assert_eq!(name, "Feishu ou");
    }

    #[tokio::test]
    async fn a_first_message_provisions_one_member_and_the_second_reuses_it() {
        let store = Store::connect_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let workspace = cowbot_core::identity::Workspace::new("Team");
        store.create_workspace(&workspace).await.unwrap();

        let integration = create_integration(&store, &workspace.id, IntegrationKind::Slack).await;
        let author = ExternalAuthor {
            external_id: "U024".into(),
            display_name: "Ada".into(),
            is_bot: false,
        };

        let first = resolve_principal(&store, &integration, &author)
            .await
            .unwrap();
        assert!(first.provisioned);
        assert_eq!(first.display_name, "Ada");

        let second = resolve_principal(&store, &integration, &author)
            .await
            .unwrap();
        assert!(!second.provisioned);
        assert_eq!(first.user_id, second.user_id);

        // One account, one row, and it cannot sign in.
        let user = store.get_user(&first.user_id).await.unwrap().unwrap();
        assert!(user.password_hash.is_empty());
        assert!(store.list_users().await.unwrap().len() == 1);
    }

    #[tokio::test]
    async fn a_renamed_account_updates_the_name_it_is_shown_under() {
        let store = Store::connect_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let workspace = cowbot_core::identity::Workspace::new("Team");
        store.create_workspace(&workspace).await.unwrap();
        let integration = create_integration(&store, &workspace.id, IntegrationKind::Slack).await;

        let author = ExternalAuthor {
            external_id: "U024".into(),
            display_name: "Ada".into(),
            is_bot: false,
        };
        let first = resolve_principal(&store, &integration, &author)
            .await
            .unwrap();
        let renamed = ExternalAuthor {
            display_name: "Ada Lovelace".into(),
            ..author
        };

        let second = resolve_principal(&store, &integration, &renamed)
            .await
            .unwrap();
        assert_eq!(second.user_id, first.user_id);
        assert_eq!(second.display_name, "Ada Lovelace");
        assert_eq!(
            store.get_user(&first.user_id).await.unwrap().unwrap().name,
            "Ada Lovelace"
        );
    }
}
