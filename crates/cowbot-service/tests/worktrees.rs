use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cowbot_bus::EventBus;
use cowbot_core::event::EventPayload;
use cowbot_core::identity::WorkspaceId;
use cowbot_core::task::Task;
use cowbot_core::worktree::WorktreeStatus;
use cowbot_service::{bootstrap_local, slug_for, WorktreeService};
use cowbot_store::Store;

async fn test_store() -> Store {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    store
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn init_repo(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    fs::write(repo.join("README.md"), "initial\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "initial"]);
    repo
}

async fn setup() -> (
    Store,
    EventBus,
    WorktreeService,
    WorkspaceId,
    PathBuf,
    tempfile::TempDir,
) {
    let store = test_store().await;
    let context = bootstrap_local(&store).await.unwrap();
    let bus = EventBus::default_bus();
    let service = WorktreeService::new(store.clone(), bus.clone());
    let temp = tempfile::tempdir().unwrap();
    let repo = init_repo(temp.path());
    (store, bus, service, context.workspace.id, repo, temp)
}

async fn make_task(store: &Store, workspace_id: &WorkspaceId, description: &str) -> Task {
    let task = Task::new(description, "A verified change");
    store.create_task(workspace_id, None, &task).await.unwrap();
    task
}

#[tokio::test]
async fn worktree_lifecycle_creates_inspects_and_discards() {
    let (store, bus, service, workspace_id, repo, _temp) = setup().await;
    let task = make_task(&store, &workspace_id, "Fix the login form").await;

    let requested = service
        .request(&task.id, "project-1", repo.to_str().unwrap(), "main")
        .await
        .unwrap();
    assert_eq!(requested.status, WorktreeStatus::Requested);
    assert!(requested.branch.is_empty());

    let ready = service.prepare(&task.id).await.unwrap();
    assert_eq!(ready.status, WorktreeStatus::Ready);
    assert!(ready.branch.starts_with("cowbot/fix-the-login-form-"));
    let worktree = PathBuf::from(&ready.worktree_path);
    assert!(worktree.is_dir());
    assert_eq!(
        worktree.file_name().unwrap().to_str().unwrap(),
        ready.branch.trim_start_matches("cowbot/")
    );
    assert!(worktree.starts_with(&repo));
    assert!(worktree.starts_with(repo.join(".cowbot").join("worktrees")));
    let exclude = fs::read_to_string(repo.join(".git").join("info").join("exclude")).unwrap();
    assert!(
        exclude.lines().any(|line| line.trim() == ".cowbot/"),
        "exclude file: {exclude}"
    );

    // Preparing an existing ready worktree is idempotent.
    let again = service.prepare(&task.id).await.unwrap();
    assert_eq!(again.branch, ready.branch);
    assert_eq!(again.worktree_path, ready.worktree_path);

    fs::write(worktree.join("README.md"), "changed\n").unwrap();
    fs::write(worktree.join("new.txt"), "new file\n").unwrap();
    let diff = service.diff(&task.id).await.unwrap();
    assert!(diff.status.contains("README.md"), "{}", diff.status);
    assert!(diff.status.contains("new.txt"), "{}", diff.status);
    assert!(diff.stat.contains("README.md"), "{}", diff.stat);
    assert!(diff.patch.contains("+changed"), "{}", diff.patch);
    assert!(!diff.truncated);

    let removed = service.discard(&task.id).await.unwrap();
    assert_eq!(removed.status, WorktreeStatus::Removed);
    assert!(!worktree.exists());
    let branches = git(&repo, &["branch", "--list", &ready.branch]);
    assert!(branches.is_empty(), "branch still exists: {branches}");
    assert!(service.discard(&task.id).await.unwrap().status == WorktreeStatus::Removed);

    let statuses = bus
        .history()
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::WorktreeUpdated { task_id, status } if task_id == &task.id => {
                Some(*status)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        statuses,
        vec![
            WorktreeStatus::Requested,
            WorktreeStatus::Ready,
            WorktreeStatus::Removed
        ]
    );
}

#[tokio::test]
async fn worktree_prepare_reports_actionable_errors() {
    let (store, _bus, service, workspace_id, repo, _temp) = setup().await;

    let task = make_task(&store, &workspace_id, "Plain folder").await;
    let plain = repo.parent().unwrap().join("plain");
    fs::create_dir_all(&plain).unwrap();
    service
        .request(&task.id, "project-1", plain.to_str().unwrap(), "main")
        .await
        .unwrap();
    let error = service.prepare(&task.id).await.unwrap_err().to_string();
    assert!(error.contains("not a git repository"), "{error}");

    let task = make_task(&store, &workspace_id, "Unknown base").await;
    service
        .request(
            &task.id,
            "project-1",
            repo.to_str().unwrap(),
            "missing-branch",
        )
        .await
        .unwrap();
    let error = service.prepare(&task.id).await.unwrap_err().to_string();
    assert!(
        error.contains("base branch 'missing-branch' does not exist"),
        "{error}"
    );
    assert!(error.contains("main"), "{error}");
}

#[tokio::test]
async fn missing_worktrees_are_reconciled_and_can_be_discarded() {
    let (store, _bus, service, workspace_id, repo, _temp) = setup().await;
    let task = make_task(&store, &workspace_id, "Vanishing worktree").await;
    service
        .request(&task.id, "project-1", repo.to_str().unwrap(), "main")
        .await
        .unwrap();
    let ready = service.prepare(&task.id).await.unwrap();

    fs::remove_dir_all(&ready.worktree_path).unwrap();
    let changed = service.reconcile().await.unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].status, WorktreeStatus::Missing);
    assert_eq!(
        service.get(&task.id).await.unwrap().unwrap().status,
        WorktreeStatus::Missing
    );

    let removed = service.discard(&task.id).await.unwrap();
    assert_eq!(removed.status, WorktreeStatus::Removed);
    let branches = git(&repo, &["branch", "--list", &ready.branch]);
    assert!(branches.is_empty(), "branch still exists: {branches}");
}

#[tokio::test]
async fn slugs_are_bounded_and_unique_per_task() {
    let task = Task::new(
        "Fix: the LOGIN form! (again) and again and again and again",
        "A fix",
    );
    let slug = slug_for(&task);
    assert!(slug.starts_with("fix-the-login-form-again-and"), "{slug}");
    assert!(
        slug.chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-'),
        "{slug}"
    );
    let (description, suffix) = slug.rsplit_once('-').unwrap();
    assert!(description.chars().count() <= 32, "{slug}");
    assert_eq!(suffix.len(), 4, "{slug}");
    assert!(task.id.0.starts_with(suffix), "{slug}");

    let other = Task::new("Fix: the LOGIN form!", "A fix");
    assert_ne!(slug_for(&other), slug);
}
