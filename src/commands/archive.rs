use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::adapters::git::{GitAdapter, GitInventory};
use crate::config::{Config, ConfigMutation};
use crate::model::{BranchId, GraphiteProvenance, RepositorySnapshot, RepositoryState};
use crate::refresh::builder::SnapshotBuilder;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveRequest {
    pub repository: PathBuf,
    pub dry_run: bool,
    pub branches: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveDisposition {
    Archived,
    WouldArchive,
    Unchanged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveResult {
    pub branch: BranchId,
    pub disposition: ArchiveDisposition,
}

struct StableEvidence {
    snapshot: Arc<RepositorySnapshot>,
    source_token: u64,
}

pub fn execute(request: ArchiveRequest) -> Result<Vec<ArchiveResult>> {
    let targets = deduplicate_targets(request.branches)?;
    let adapter = GitAdapter::discover(&request.repository).with_context(|| {
        format!(
            "{} is not inside a readable Git repository; nothing changed",
            request.repository.display()
        )
    })?;
    let initial = stable_evidence(&adapter)
        .context("could not establish stable archive evidence; nothing changed")?;
    validate_targets(&initial.snapshot, &targets).map_err(nothing_changed)?;
    let initial_config = Config::load(&initial.snapshot.common_dir)
        .context("Stackmap config is invalid; nothing changed")?;
    let already_archived: HashSet<_> = targets
        .iter()
        .filter(|target| initial_config.is_archived(target))
        .cloned()
        .collect();

    if request.dry_run {
        return Ok(results_for(&targets, &already_archived, true));
    }

    let mut mutation = ConfigMutation::default();
    for target in &targets {
        mutation.set_archived(target.clone(), true);
    }

    let final_evidence = stable_evidence(&adapter)
        .context("could not revalidate archive evidence; nothing changed")?;
    ensure_unchanged(&initial, &final_evidence).map_err(nothing_changed)?;
    validate_targets(&final_evidence.snapshot, &targets).map_err(nothing_changed)?;
    Config::persist_mutation_strict(&initial.snapshot.common_dir, &mutation, |_| Ok(()))
        .context("archive config was not saved; nothing changed")?;

    Ok(results_for(&targets, &already_archived, false))
}

fn deduplicate_targets(branches: Vec<String>) -> Result<Vec<BranchId>> {
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for branch in branches {
        if branch.is_empty() {
            bail!("branch names cannot be empty; nothing changed");
        }
        let branch = BranchId::new(branch);
        if seen.insert(branch.clone()) {
            targets.push(branch);
        }
    }
    if targets.is_empty() {
        bail!("archive requires at least one branch; nothing changed");
    }
    Ok(targets)
}

fn stable_evidence(adapter: &GitAdapter) -> Result<StableEvidence> {
    for _ in 0..3 {
        let mut builder = SnapshotBuilder::new(adapter.clone());
        let snapshot = builder.build()?;
        let inventory = adapter.inventory()?;
        if snapshot_matches_inventory(&snapshot, &inventory) {
            return Ok(StableEvidence {
                snapshot,
                source_token: inventory.source_token,
            });
        }
    }
    bail!("repository changed during three archive evidence attempts")
}

fn snapshot_matches_inventory(snapshot: &RepositorySnapshot, inventory: &GitInventory) -> bool {
    if snapshot.root != inventory.root
        || snapshot.git_dir != inventory.git_dir
        || snapshot.common_dir != inventory.common_dir
        || snapshot.repository_id != inventory.repository_id
        || snapshot.state != inventory.state
        || snapshot.branches.len() != inventory.branches.len()
        || snapshot
            .branches
            .iter()
            .find(|branch| branch.current)
            .map(|branch| &branch.id)
            != inventory.current.as_ref()
    {
        return false;
    }
    snapshot.branches.iter().all(|branch| {
        inventory
            .branches
            .iter()
            .find(|candidate| candidate.id == branch.id)
            .is_some_and(|candidate| candidate.oid == branch.oid)
    })
}

fn validate_targets(snapshot: &RepositorySnapshot, targets: &[BranchId]) -> Result<()> {
    if !matches!(snapshot.state, RepositoryState::Ready) {
        bail!("repository is not ready ({:?})", snapshot.state);
    }
    for target in targets {
        let branch = snapshot
            .branch(target)
            .ok_or_else(|| anyhow::anyhow!("branch {target} does not exist"))?;
        if branch.current {
            bail!("{target} is current and cannot be archived");
        }
        if snapshot.configured_trunks.contains(target) || snapshot.trunks.contains(target) {
            bail!("{target} is a trunk and cannot be archived");
        }
        if branch.graphite == GraphiteProvenance::Degraded {
            bail!("{target} has unsafe Graphite topology evidence");
        }
    }
    Ok(())
}

fn ensure_unchanged(initial: &StableEvidence, current: &StableEvidence) -> Result<()> {
    if initial.snapshot.repository_id != current.snapshot.repository_id
        || initial.snapshot.common_dir != current.snapshot.common_dir
    {
        bail!("repository identity changed before archive persistence");
    }
    if initial.source_token != current.source_token {
        bail!("repository source token changed before archive persistence");
    }
    if initial.snapshot.configured_trunks != current.snapshot.configured_trunks
        || initial.snapshot.trunks != current.snapshot.trunks
    {
        bail!("repository trunks changed before archive persistence");
    }
    Ok(())
}

fn results_for(
    targets: &[BranchId],
    already_archived: &HashSet<BranchId>,
    dry_run: bool,
) -> Vec<ArchiveResult> {
    targets
        .iter()
        .map(|branch| ArchiveResult {
            branch: branch.clone(),
            disposition: if already_archived.contains(branch) {
                ArchiveDisposition::Unchanged
            } else if dry_run {
                ArchiveDisposition::WouldArchive
            } else {
                ArchiveDisposition::Archived
            },
        })
        .collect()
}

fn nothing_changed(error: anyhow::Error) -> anyhow::Error {
    anyhow::anyhow!("{error}; nothing changed")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use rusqlite::Connection;

    use super::*;
    use crate::config::config_path;

    fn git(cwd: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn repository() -> tempfile::TempDir {
        let repository = tempfile::tempdir().unwrap();
        git(repository.path(), &["init", "-b", "main"]);
        git(
            repository.path(),
            &["config", "user.name", "Stackmap Tests"],
        );
        git(
            repository.path(),
            &["config", "user.email", "stackmap@example.invalid"],
        );
        fs::write(repository.path().join("file.txt"), "base\n").unwrap();
        git(repository.path(), &["add", "file.txt"]);
        git(repository.path(), &["commit", "-m", "base"]);
        git(repository.path(), &["branch", "alpha"]);
        git(repository.path(), &["branch", "beta"]);
        repository
    }

    fn request(repository: &std::path::Path, dry_run: bool, branches: &[&str]) -> ArchiveRequest {
        ArchiveRequest {
            repository: repository.to_owned(),
            dry_run,
            branches: branches.iter().map(ToString::to_string).collect(),
        }
    }

    #[test]
    fn target_deduplication_preserves_first_seen_order() {
        let targets = deduplicate_targets(vec!["b".into(), "a".into(), "b".into()]).unwrap();
        assert_eq!(targets, vec![BranchId::new("b"), BranchId::new("a")]);
    }

    #[test]
    fn archives_multiple_git_only_branches_atomically_and_is_idempotent() {
        let repository = repository();
        let refs_before = git(repository.path(), &["show-ref"]);
        let head_before = git(repository.path(), &["rev-parse", "HEAD"]);

        let first = execute(request(
            repository.path(),
            false,
            &["beta", "alpha", "beta"],
        ))
        .unwrap();
        assert_eq!(
            first,
            vec![
                ArchiveResult {
                    branch: BranchId::new("beta"),
                    disposition: ArchiveDisposition::Archived,
                },
                ArchiveResult {
                    branch: BranchId::new("alpha"),
                    disposition: ArchiveDisposition::Archived,
                },
            ]
        );
        let common_dir = repository.path().join(".git");
        let config = Config::load(&common_dir).unwrap();
        assert!(config.is_archived(&BranchId::new("alpha")));
        assert!(config.is_archived(&BranchId::new("beta")));
        assert_eq!(git(repository.path(), &["show-ref"]), refs_before);
        assert_eq!(git(repository.path(), &["rev-parse", "HEAD"]), head_before);

        let second = execute(request(repository.path(), false, &["alpha", "beta"])).unwrap();
        assert!(
            second
                .iter()
                .all(|result| result.disposition == ArchiveDisposition::Unchanged)
        );
    }

    #[test]
    fn one_invalid_target_refuses_the_whole_batch_without_config_artifacts() {
        let repository = repository();
        let config = config_path(&repository.path().join(".git"));
        let error = execute(request(repository.path(), false, &["alpha", "missing"])).unwrap_err();
        assert!(error.to_string().contains("missing"));
        assert!(error.to_string().contains("nothing changed"));
        assert!(!config.exists());

        let current = execute(request(repository.path(), false, &["main"])).unwrap_err();
        assert!(current.to_string().contains("current"));
        assert!(!config.exists());
    }

    #[test]
    fn dry_run_validates_and_reports_without_creating_any_config_artifact() {
        let repository = repository();
        let common_dir = repository.path().join(".git");
        let config = config_path(&common_dir);
        let results = execute(request(repository.path(), true, &["alpha", "beta"])).unwrap();
        assert!(
            results
                .iter()
                .all(|result| result.disposition == ArchiveDisposition::WouldArchive)
        );
        assert!(!config.exists());
        assert!(!config.with_extension("lock").exists());
        assert!(!config.parent().unwrap().exists());
    }

    #[test]
    fn malformed_config_refuses_without_replacing_its_bytes() {
        let repository = repository();
        let config = config_path(&repository.path().join(".git"));
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        let malformed = b"not valid = [toml";
        fs::write(&config, malformed).unwrap();

        let error = execute(request(repository.path(), false, &["alpha"])).unwrap_err();

        assert!(error.to_string().contains("config is invalid"));
        assert_eq!(fs::read(config).unwrap(), malformed);
    }

    #[test]
    fn linked_worktree_uses_the_shared_common_git_directory() {
        let repository = repository();
        let linked = repository.path().join("linked-worktree");
        git(
            repository.path(),
            &["worktree", "add", "-b", "linked", linked.to_str().unwrap()],
        );

        execute(request(&linked, false, &["alpha"])).unwrap();

        let config = Config::load(&repository.path().join(".git")).unwrap();
        assert!(config.is_archived(&BranchId::new("alpha")));
        assert!(!linked.join(".git/stackmap/config.toml").exists());
    }

    #[test]
    fn configured_trunk_and_degraded_topology_targets_are_refused() {
        let repository = repository();
        let common_dir = repository.path().join(".git");
        fs::write(
            common_dir.join(".graphite_repo_config"),
            r#"{"trunk":"main"}"#,
        )
        .unwrap();
        let connection = Connection::open(common_dir.join(".graphite_metadata.db")).unwrap();
        connection
            .execute(
                "CREATE TABLE branch_metadata (branch_name TEXT, parent_branch_name TEXT)",
                [],
            )
            .unwrap();
        connection
            .execute("INSERT INTO branch_metadata VALUES ('main', NULL)", [])
            .unwrap();
        connection
            .execute("INSERT INTO branch_metadata VALUES ('alpha', 'main')", [])
            .unwrap();
        drop(connection);
        git(repository.path(), &["switch", "alpha"]);

        let trunk = execute(request(repository.path(), false, &["main"])).unwrap_err();
        assert!(trunk.to_string().contains("trunk"));
        assert!(!config_path(&common_dir).exists());

        fs::write(common_dir.join(".graphite_repo_config"), b"not json").unwrap();
        let degraded = execute(request(repository.path(), false, &["beta"])).unwrap_err();
        assert!(degraded.to_string().contains("unsafe Graphite topology"));
        assert!(!config_path(&common_dir).exists());
    }

    #[test]
    fn source_token_drift_is_detected_before_persistence() {
        let repository = repository();
        let adapter = GitAdapter::discover(repository.path()).unwrap();
        let initial = stable_evidence(&adapter).unwrap();
        git(repository.path(), &["branch", "later"]);
        let changed = stable_evidence(&adapter).unwrap();

        let error = ensure_unchanged(&initial, &changed).unwrap_err();

        assert!(error.to_string().contains("source token changed"));
    }
}
