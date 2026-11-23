//! Attachments: a file somebody sent in the IM, on disk where a tool can reach
//! it (ADR-0060).
//!
//! The honest scope comes first, because it is easy to claim more than this
//! delivers. **A model in this build cannot be sent an image.**
//! `ChatMessage.content` is a `String` in every provider client
//! (`cowbot-llm/src/types.rs`), so there is no content-part type to put a
//! picture into, and nothing in the three protocol encoders would carry one. What
//! a turn *can* do is run a tool. So an attachment is written below the
//! directory the deployment's file tools are rooted at, and its path is named in
//! the message text. An agent reads a text file with `read_file` and inspects any
//! other with `execute_code`; a whiteboard photo is stored and named, not
//! understood. Image understanding is a change to the LLM type layer and all
//! three encoders, and it is not this milestone.
//!
//! Three rules are not negotiable, because the bytes come from a stranger:
//!
//! * **The stored name is generated, never the sender's.** A name that reached a
//!   filesystem path could escape the root, and `../../etc/passwd` is the first
//!   thing anyone tries.
//! * **The size is capped while the bytes arrive**, not after: buffering a
//!   two-gigabyte file to reject it is the denial of service the cap exists to
//!   prevent.
//! * **Fetching happens after the dedupe claim.** A platform retry that
//!   downloaded first would pay for the same file twice.
//!
//! One property that is *not* claimed: the file tools are rooted, not confined.
//! `cowbot-tools/src/filesystem.rs` says so — an absolute path is returned as-is
//! and `..` is not refused. Storing an attachment below the root protects the
//! host from the *platform* choosing a write path. It does not contain an agent
//! that decided to walk out of it, which is the approval gate's job.

use std::path::{Path, PathBuf};

use cowbot_core::error::{CowError, CowResult};
use cowbot_core::integration::IntegrationId;
use tracing::debug;

use crate::adapter::Integration;
use crate::model::{Attachment, AttachmentKind, InboundMessage};

/// The largest attachment this build stores.
///
/// Twenty megabytes is Telegram's own bot-download ceiling, so no platform is
/// more permissive than the pipeline. The bytes go to disk rather than into a
/// prompt, so this bounds disk and download time rather than context.
pub const MAX_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;

/// How many files one message may carry.
///
/// An album of sixty photos is one delivery and sixty files; the surplus is
/// named in the text as not kept, so an agent is told the message had more than
/// it can see rather than quietly given the first four.
pub const MAX_ATTACHMENTS_PER_MESSAGE: usize = 4;

/// The directory, under the deployment's root, that attachments live in.
pub const ATTACHMENTS_DIR: &str = "attachments";

/// The longest part of a name that survives sanitising.
const MAX_NAME_CHARS: usize = 80;

/// One attachment that was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    /// Path relative to the deployment's root, which is what a rooted file tool
    /// accepts verbatim.
    pub path: String,
    pub bytes: u64,
}

/// Where inbound attachment bytes land.
///
/// The root is the same directory the deployment's file tools and sandbox are
/// rooted at, so a path this produces is one a tool can open without the model
/// being told about absolute server paths.
#[derive(Debug, Clone)]
pub struct AttachmentStore {
    root: PathBuf,
}

impl AttachmentStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Fetch one attachment and write it below the root.
    ///
    /// Returns an error whose message is shown to the reader in the message text
    /// — "this adapter cannot download attachments", "it is above the limit" — so
    /// the failure is legible rather than a missing file with no explanation.
    pub async fn store(
        &self,
        integration: &IntegrationId,
        adapter: &dyn Integration,
        attachment: &Attachment,
    ) -> CowResult<Stored> {
        if let Some(size) = attachment.size_bytes {
            if size > MAX_ATTACHMENT_BYTES {
                // Refused before the download: an obviously oversized file must
                // not cost the bandwidth it would take to discover that.
                return Err(CowError::Config(format!(
                    "it is {size} bytes, above the {MAX_ATTACHMENT_BYTES}-byte limit"
                )));
            }
        }

        let bytes = adapter
            .fetch_attachment(attachment, MAX_ATTACHMENT_BYTES)
            .await?;
        if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
            // The adapter's own cap is its business; this is the backstop for one
            // that ignored it, and it runs before anything is written.
            return Err(CowError::Config(format!(
                "it is {} bytes, above the {MAX_ATTACHMENT_BYTES}-byte limit",
                bytes.len()
            )));
        }

        let directory = self.directory(integration);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|error| {
                CowError::Storage(format!("could not create {directory:?}: {error}"))
            })?;

        let file_name = file_name(attachment);
        let path = directory.join(&file_name);
        write_new(&path, &bytes).await?;

        // Belt to the traversal braces: a symlinked directory a previous turn
        // created with `execute_code` would otherwise let this write land
        // somewhere else entirely.
        contain(&self.root, &path)?;

        let relative = Path::new(ATTACHMENTS_DIR)
            .join(safe_component(&integration.0))
            .join(&file_name)
            .to_string_lossy()
            .replace('\\', "/");
        debug!(path = %relative, bytes = bytes.len(), "stored an IM attachment");
        Ok(Stored {
            path: relative,
            bytes: bytes.len() as u64,
        })
    }

    /// The directory one integration's attachments live in.
    fn directory(&self, integration: &IntegrationId) -> PathBuf {
        self.root
            .join(ATTACHMENTS_DIR)
            .join(safe_component(&integration.0))
    }
}

/// Write a file that does not already exist.
///
/// `create_new` rather than `write`: an attachment must never replace a file a
/// previous turn wrote, and the unique name makes a collision exceptional enough
/// that failing on it is the right answer.
async fn write_new(path: &Path, bytes: &[u8]) -> CowResult<()> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
        .map_err(|error| CowError::Storage(format!("could not create {path:?}: {error}")))?;
    file.write_all(bytes)
        .await
        .map_err(|error| CowError::Storage(format!("could not write {path:?}: {error}")))?;
    file.flush()
        .await
        .map_err(|error| CowError::Storage(format!("could not write {path:?}: {error}")))?;
    Ok(())
}

/// Refuse a path that resolved outside `root`.
///
/// Runs after the write on purpose. A canonicalized parent that is not below the
/// root means the write already went somewhere unexpected, and the honest
/// response is to remove the file and report — checking first would leave a
/// window between the check and the write.
fn contain(root: &Path, path: &Path) -> CowResult<()> {
    let (Ok(root), Ok(parent)) = (root.canonicalize(), path.canonicalize()) else {
        // A root or a path that cannot be canonicalized is not a claim this can
        // check; the write itself already succeeded, so the attachment stands.
        return Ok(());
    };
    if parent.starts_with(&root) {
        return Ok(());
    }
    let _ = std::fs::remove_file(path);
    Err(CowError::Storage(format!(
        "the attachment resolved outside the workspace root and was removed: {parent:?}"
    )))
}

/// The generated name for one attachment.
///
/// The platform's id first so two files of the same name in one chat do not
/// collide, then the sender's name sanitised to `[A-Za-z0-9._-]`. The extension
/// survives because it is what tells a tool what the bytes are.
pub fn file_name(attachment: &Attachment) -> String {
    let unique = safe_component(&attachment.external_id);
    let sender = attachment.file_name.as_deref().unwrap_or_default();
    let tail = sender.rsplit(['/', '\\']).next().unwrap_or(sender).trim();
    let mut cleaned = String::with_capacity(tail.len());
    for character in tail.chars().take(MAX_NAME_CHARS) {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
            cleaned.push(character);
        } else {
            cleaned.push('_');
        }
    }
    let cleaned = cleaned.trim_matches('.').to_string();
    if !unique.is_empty() {
        return if cleaned.is_empty() {
            format!("{unique}.{}", extension_for(attachment.kind))
        } else {
            format!("{unique}-{cleaned}")
        };
    }
    if cleaned.is_empty() {
        format!("attachment.{}", extension_for(attachment.kind))
    } else {
        cleaned
    }
}

/// One path component that cannot mean anything but itself.
///
/// Only `[A-Za-z0-9._-]` survive, so no separator, drive letter, or `..` can be
/// built from it. An empty result becomes `_`, because a component of nothing
/// would join to its parent.
fn safe_component(candidate: &str) -> String {
    let cleaned: String = candidate
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
        .collect();
    if cleaned.trim_matches('.').is_empty() {
        "_".to_string()
    } else {
        cleaned
    }
}

fn extension_for(kind: AttachmentKind) -> &'static str {
    match kind {
        AttachmentKind::Image => "png",
        AttachmentKind::Audio => "bin",
        AttachmentKind::Video => "bin",
        AttachmentKind::File => "bin",
    }
}

/// Describe the attachments a message carried, for the text a turn reads.
///
/// This is what makes a file usable: an agent told "there was an image" can only
/// guess, while one told the path can read it.
pub fn describe<'a>(
    stored: impl Iterator<Item = (&'a Attachment, Option<&'a Stored>)>,
) -> Option<String> {
    let mut lines = Vec::new();
    for (attachment, written) in stored {
        let name = attachment
            .file_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or("unnamed");
        match written {
            Some(stored) => lines.push(format!(
                "[{}] {name}: {} ({})",
                attachment.kind.as_str(),
                stored.path,
                attachment.mime_type.as_deref().unwrap_or("unknown type")
            )),
            None => lines.push(format!("[{}] {name}: not kept", attachment.kind.as_str())),
        }
    }
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "\n\n---\nFiles sent with this message:\n{}",
        lines.join("\n")
    ))
}

/// What the pipeline needs to store the files on one message.
///
/// A bundle rather than two parameters because the two are only ever useful
/// together: the store says where, the adapter says how. `None` for a deployment
/// with no root, which is how a sink without attachment support is expressed.
pub struct Inbound<'a> {
    pub store: &'a AttachmentStore,
    pub adapter: &'a dyn Integration,
    pub integration: &'a IntegrationId,
}

/// Store every attachment on `message`, returning the text that names them.
///
/// Order matters and is the caller's to get right: this runs *after* the dedupe
/// claim, so a platform retry downloads nothing. A file that cannot be stored is
/// named as not kept rather than dropping the message — a question with a broken
/// picture is still a question.
pub async fn store_all(message: &InboundMessage, sink: Option<&Inbound<'_>>) -> Option<String> {
    let surplus = message
        .attachments
        .len()
        .saturating_sub(MAX_ATTACHMENTS_PER_MESSAGE);
    let mut written: Vec<Stored> = Vec::new();
    let mut outcomes: Vec<Option<Stored>> = Vec::new();

    for (index, attachment) in message.attachments.iter().enumerate() {
        if index >= MAX_ATTACHMENTS_PER_MESSAGE {
            outcomes.push(None);
            continue;
        }
        let Some(sink) = sink else {
            outcomes.push(None);
            continue;
        };
        match sink
            .store
            .store(sink.integration, sink.adapter, attachment)
            .await
        {
            Ok(stored) => {
                written.push(stored.clone());
                outcomes.push(Some(stored));
            }
            Err(error) => {
                debug!(
                    external = %attachment.external_id,
                    %error,
                    "could not store an IM attachment"
                );
                outcomes.push(None);
            }
        }
    }

    let mut described = describe(
        message
            .attachments
            .iter()
            .zip(outcomes.iter())
            .map(|(attachment, stored)| (attachment, stored.as_ref())),
    )?;
    if surplus > 0 {
        described.push_str(&format!(
            "\n{} more file(s) were sent than this build keeps.",
            surplus
        ));
    }
    Some(described)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DeliveredId, OutboundContent, OutboundTarget};
    use async_trait::async_trait;
    use cowbot_core::cancel::CancelSignal;
    use cowbot_core::integration::IntegrationKind;

    fn attachment(name: Option<&str>, external_id: &str, size: Option<u64>) -> Attachment {
        Attachment {
            external_id: external_id.into(),
            kind: AttachmentKind::File,
            file_name: name.map(str::to_string),
            mime_type: Some("text/csv".into()),
            size_bytes: size,
        }
    }

    /// An adapter that returns fixed bytes.
    struct Bytes {
        bytes: Vec<u8>,
        fail: bool,
    }

    #[async_trait]
    impl Integration for Bytes {
        fn kind(&self) -> IntegrationKind {
            IntegrationKind::Slack
        }
        async fn run(
            &self,
            _ctx: crate::adapter::IntegrationContext,
            cancel: CancelSignal,
        ) -> CowResult<()> {
            cancel.cancelled().await;
            Ok(())
        }
        async fn deliver(
            &self,
            _target: &OutboundTarget,
            _content: &OutboundContent,
        ) -> CowResult<DeliveredId> {
            Ok(DeliveredId("1".into()))
        }
        async fn fetch_attachment(
            &self,
            _attachment: &Attachment,
            _max: u64,
        ) -> CowResult<Vec<u8>> {
            if self.fail {
                return Err(CowError::Config("the file is gone".into()));
            }
            Ok(self.bytes.clone())
        }
    }

    fn integration() -> IntegrationId {
        IntegrationId::new("i1")
    }

    #[test]
    fn a_senders_name_cannot_escape_the_directory() {
        // The attack a generated name exists to stop.
        let name = file_name(&attachment(Some("../../etc/passwd"), "F1", Some(3)));
        assert!(!name.contains('/'), "{name}");
        assert!(!name.contains('\\'), "{name}");
        assert!(name.ends_with("passwd"), "{name}");

        let windows = file_name(&attachment(Some("C:\\Windows\\cmd.exe"), "F2", Some(3)));
        assert!(!windows.contains(':'), "{windows}");
        assert!(windows.ends_with("cmd.exe"), "{windows}");
    }

    #[test]
    fn the_platform_id_keeps_two_files_of_one_name_apart() {
        let first = file_name(&attachment(Some("report.csv"), "F1", Some(3)));
        let second = file_name(&attachment(Some("report.csv"), "F2", Some(3)));
        assert_ne!(first, second);
        assert!(first.ends_with("report.csv"), "{first}");
    }

    #[test]
    fn a_name_of_dots_or_nothing_still_produces_a_file() {
        assert!(file_name(&attachment(Some(".."), "F1", Some(1))).ends_with(".bin"));
        assert!(file_name(&attachment(None, "F1", Some(1))).ends_with(".bin"));
        assert!(!file_name(&attachment(None, "", Some(1))).contains(".."));
    }

    #[test]
    fn a_non_ascii_name_keeps_its_extension() {
        let name = file_name(&attachment(Some("部署日志.log"), "F1", Some(1)));
        assert!(name.ends_with(".log"), "{name}");
        assert!(name.is_ascii(), "{name}");
    }

    #[test]
    fn a_path_component_cannot_be_a_parent_reference() {
        assert_eq!(safe_component(".."), "_");
        assert_eq!(safe_component("a/b"), "ab");
        assert_eq!(safe_component(""), "_");
    }

    #[tokio::test]
    async fn a_stored_file_lands_below_the_root_and_below_its_integration() {
        let directory = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(directory.path());
        let adapter = Bytes {
            bytes: b"a,b\n1,2".to_vec(),
            fail: false,
        };

        let stored = store
            .store(
                &integration(),
                &adapter,
                &attachment(Some("rows.csv"), "F1", Some(8)),
            )
            .await
            .unwrap();

        assert!(
            stored.path.starts_with("attachments/i1/"),
            "{}",
            stored.path
        );
        assert_eq!(stored.bytes, 7);
        assert!(directory.path().join(&stored.path).exists());
    }

    #[tokio::test]
    async fn an_oversized_attachment_is_refused_before_it_is_fetched() {
        // The adapter is not even asked: the declared size decides.
        let directory = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(directory.path());
        let adapter = Bytes {
            bytes: b"should not be asked for".to_vec(),
            fail: true,
        };

        let error = store
            .store(
                &integration(),
                &adapter,
                &attachment(Some("huge.bin"), "F1", Some(MAX_ATTACHMENT_BYTES + 1)),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("above the"), "{error}");
    }

    #[tokio::test]
    async fn a_file_the_adapter_cannot_reach_reports_why() {
        let directory = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(directory.path());
        let adapter = Bytes {
            bytes: Vec::new(),
            fail: true,
        };
        let error = store
            .store(
                &integration(),
                &adapter,
                &attachment(Some("a.csv"), "F1", None),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("gone"), "{error}");
    }

    #[tokio::test]
    async fn a_second_file_of_the_same_name_does_not_overwrite_the_first() {
        let directory = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(directory.path());
        let adapter = Bytes {
            bytes: b"one".to_vec(),
            fail: false,
        };
        let first = store
            .store(
                &integration(),
                &adapter,
                &attachment(Some("a.csv"), "F1", None),
            )
            .await
            .unwrap();
        let second = store
            .store(
                &integration(),
                &adapter,
                &attachment(Some("a.csv"), "F2", None),
            )
            .await
            .unwrap();
        assert_ne!(first.path, second.path);
        assert_eq!(
            tokio::fs::read(directory.path().join(&first.path))
                .await
                .unwrap(),
            b"one"
        );
    }

    #[test]
    fn the_description_names_the_path_so_a_tool_can_open_it() {
        let attachment = attachment(Some("rows.csv"), "F1", Some(3));
        let stored = Stored {
            path: "attachments/i1/F1-rows.csv".into(),
            bytes: 3,
        };
        let described = describe(std::iter::once((&attachment, Some(&stored)))).unwrap();
        assert!(
            described.contains("attachments/i1/F1-rows.csv"),
            "{described}"
        );
        assert!(described.contains("text/csv"), "{described}");
    }

    #[test]
    fn a_file_that_was_not_kept_says_so_rather_than_naming_a_path() {
        let attachment = attachment(Some("rows.csv"), "F1", Some(3));
        let described = describe(std::iter::once((&attachment, None))).unwrap();
        assert!(described.contains("not kept"), "{described}");
        assert!(!described.contains("attachments/"), "{described}");
    }

    #[test]
    fn a_message_with_no_attachments_adds_nothing() {
        assert!(describe(std::iter::empty()).is_none());
    }
}
