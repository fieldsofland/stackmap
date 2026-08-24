use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use super::archive::{ensure_unchanged, stable_evidence};
use crate::adapters::git::GitAdapter;
use crate::config::{Config, ConfigMutation};
use crate::model::topology::TopologyIndex;
use crate::model::{BranchId, GraphiteProvenance, RepositorySnapshot, RepositoryState};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StackRenameRequest {
    pub repository: PathBuf,
    pub dry_run: bool,
    pub branch: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StackRenameDisposition {
    Renamed,
    WouldRename,
    Unchanged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StackRenameResult {
    pub stack: BranchId,
    pub name: Arc<str>,
    pub disposition: StackRenameDisposition,
}

pub fn rename(request: StackRenameRequest) -> Result<StackRenameResult> {
    let target = BranchId::new(request.branch);
    if request.name.trim().is_empty() {
        bail!("stack names cannot be empty; nothing changed");
    }
    if request.name.trim() != request.name {
        bail!("stack names cannot start or end with whitespace; nothing changed");
    }
    let name: Arc<str> = Arc::from(request.name);
    let adapter = GitAdapter::discover(&request.repository).with_context(|| {
        format!(
            "{} is not inside a readable Git repository; nothing changed",
            request.repository.display()
        )
    })?;
    let initial = stable_evidence(&adapter)
        .context("could not establish stable stack rename evidence; nothing changed")?;
    let stack = resolve_stack(&initial.snapshot, &target).map_err(nothing_changed)?;
    let initial_config = Config::load(&initial.snapshot.common_dir)
        .context("Stackmap config is invalid; nothing changed")?;
    let mut validated = initial_config.clone();
    validated
        .set_stack_name_in_memory(&stack, Some(&name))
        .map_err(nothing_changed)?;
    let previous_name = initial_config.stack_name(&stack).map(str::to_owned);
    if previous_name.as_deref() == Some(name.as_ref()) {
        return Ok(result(stack, name, StackRenameDisposition::Unchanged));
    }
    if request.dry_run {
        return Ok(result(stack, name, StackRenameDisposition::WouldRename));
    }

    let final_evidence = stable_evidence(&adapter)
        .context("could not revalidate stack rename evidence; nothing changed")?;
    ensure_unchanged(&initial, &final_evidence).map_err(nothing_changed)?;
    let final_stack = resolve_stack(&final_evidence.snapshot, &target).map_err(nothing_changed)?;
    if final_stack != stack {
        bail!("displayed stack identity changed before config persistence; nothing changed");
    }

    let mut mutation = ConfigMutation::default();
    mutation.set_stack_name(stack.clone(), Some(name.clone()));
    let saved =
        Config::persist_mutation_strict(&initial.snapshot.common_dir, &mutation, |latest| {
            if latest.stack_name(&stack) != previous_name.as_deref() {
                bail!("stack name changed concurrently")
            }
            Ok(())
        })
        .context("stack name was not saved; nothing changed")?;
    if saved.stack_name(&stack) != Some(name.as_ref()) {
        bail!("saved stack name could not be verified; nothing changed");
    }

    Ok(result(stack, name, StackRenameDisposition::Renamed))
}

fn resolve_stack(snapshot: &RepositorySnapshot, target: &BranchId) -> Result<BranchId> {
    if !matches!(snapshot.state, RepositoryState::Ready) {
        bail!("repository is not ready ({:?})", snapshot.state);
    }
    let branch = snapshot
        .branch(target)
        .ok_or_else(|| anyhow::anyhow!("branch {target} does not exist"))?;
    if snapshot.configured_trunks.contains(target) || snapshot.trunks.contains(target) {
        bail!("{target} is a trunk and cannot be named");
    }
    if branch.graphite == GraphiteProvenance::Degraded {
        bail!("{target} has unsafe Graphite topology evidence");
    }
    let topology = TopologyIndex::build(snapshot);
    let stack = topology
        .stack_for(target)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{target} does not belong to a displayed stack"))?;
    let stack_branch = snapshot
        .branch(&stack)
        .ok_or_else(|| anyhow::anyhow!("displayed stack {stack} does not exist"))?;
    if stack_branch.graphite == GraphiteProvenance::Degraded {
        bail!("{stack} has unsafe Graphite topology evidence");
    }
    Ok(stack)
}

fn result(
    stack: BranchId,
    name: Arc<str>,
    disposition: StackRenameDisposition,
) -> StackRenameResult {
    StackRenameResult {
        stack,
        name,
        disposition,
    }
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
        git(repository.path(), &["branch", "gamma"]);
        repository
    }

    fn request(repository: &std::path::Path, branch: &str, name: &str) -> StackRenameRequest {
        StackRenameRequest {
            repository: repository.to_owned(),
            dry_run: false,
            branch: branch.to_owned(),
            name: name.to_owned(),
        }
    }

    #[test]
    fn renames_a_git_only_stack_without_changing_git() {
        let repository = repository();
        let refs = git(repository.path(), &["show-ref"]);
        let head = git(repository.path(), &["rev-parse", "HEAD"]);

        let result = rename(request(repository.path(), "alpha", "Payments cleanup")).unwrap();

        assert_eq!(result.stack, BranchId::new("alpha"));
        assert_eq!(result.name.as_ref(), "Payments cleanup");
        assert_eq!(result.disposition, StackRenameDisposition::Renamed);
        let config = Config::load(&repository.path().join(".git")).unwrap();
        assert_eq!(
            config.stack_name(&BranchId::new("alpha")),
            Some("Payments cleanup")
        );
        assert_eq!(git(repository.path(), &["show-ref"]), refs);
        assert_eq!(git(repository.path(), &["rev-parse", "HEAD"]), head);
    }

    #[test]
    fn members_and_forks_resolve_to_their_displayed_stack_ids() {
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
        for (branch, parent) in [
            ("main", None),
            ("alpha", Some("main")),
            ("beta", Some("alpha")),
            ("gamma", Some("alpha")),
        ] {
            connection
                .execute(
                    "INSERT INTO branch_metadata VALUES (?1, ?2)",
                    rusqlite::params![branch, parent],
                )
                .unwrap();
        }
        drop(connection);

        let member = rename(request(repository.path(), "beta", "Primary work")).unwrap();
        let fork = rename(request(repository.path(), "gamma", "Side quest")).unwrap();

        assert_eq!(member.stack, BranchId::new("alpha"));
        assert_eq!(fork.stack, BranchId::new("gamma"));
        let config = Config::load(&common_dir).unwrap();
        assert_eq!(
            config.stack_name(&BranchId::new("gamma")),
            Some("Side quest")
        );
        assert_eq!(
            config.stack_name(&BranchId::new("alpha")),
            Some("Primary work")
        );
        assert_eq!(config.stack_name(&BranchId::new("beta")), None);
    }

    #[test]
    fn dry_run_and_unchanged_do_not_write_config() {
        let repository = repository();
        let common_dir = repository.path().join(".git");
        let config_path = config_path(&common_dir);
        let mut dry_run = request(repository.path(), "alpha", "Preview");
        dry_run.dry_run = true;

        let preview = rename(dry_run).unwrap();

        assert_eq!(preview.disposition, StackRenameDisposition::WouldRename);
        assert!(!config_path.exists());
        assert!(!config_path.parent().unwrap().exists());

        rename(request(repository.path(), "alpha", "Preview")).unwrap();
        let before = fs::read(&config_path).unwrap();
        let unchanged = rename(request(repository.path(), "alpha", "Preview")).unwrap();
        assert_eq!(unchanged.disposition, StackRenameDisposition::Unchanged);
        assert_eq!(fs::read(config_path).unwrap(), before);
    }

    #[test]
    fn rejects_invalid_names_and_targets_without_writing() {
        let repository = repository();
        let config = config_path(&repository.path().join(".git"));
        for (branch, name, expected) in [
            ("alpha", "", "nothing changed"),
            ("alpha", " padded ", "whitespace"),
            ("missing", "Name", "does not exist"),
        ] {
            let error = rename(request(repository.path(), branch, name)).unwrap_err();
            assert!(error.to_string().contains(expected), "{error:#}");
            assert!(!config.exists());
        }
        fs::write(
            repository.path().join(".git/.graphite_repo_config"),
            r#"{"trunk":"main"}"#,
        )
        .unwrap();
        let connection =
            Connection::open(repository.path().join(".git/.graphite_metadata.db")).unwrap();
        connection
            .execute(
                "CREATE TABLE branch_metadata (branch_name TEXT, parent_branch_name TEXT)",
                [],
            )
            .unwrap();
        connection
            .execute("INSERT INTO branch_metadata VALUES ('main', NULL)", [])
            .unwrap();
        drop(connection);
        let error = rename(request(repository.path(), "main", "Name")).unwrap_err();
        assert!(error.to_string().contains("trunk"), "{error:#}");
        assert!(!config.exists());
    }

    #[test]
    fn malformed_config_is_not_replaced() {
        let repository = repository();
        let config = config_path(&repository.path().join(".git"));
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        let malformed = b"not valid = [toml";
        fs::write(&config, malformed).unwrap();

        let error = rename(request(repository.path(), "alpha", "Name")).unwrap_err();

        assert!(error.to_string().contains("config is invalid"));
        assert_eq!(fs::read(config).unwrap(), malformed);
    }
}
