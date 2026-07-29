use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::model::BranchId;

pub const MAX_STACK_NAME_CHARS: usize = 80;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VisualSection {
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Config {
    #[serde(default)]
    pub colors: BTreeMap<String, String>,
    #[serde(default)]
    pub archived: BTreeSet<String>,
    #[serde(default)]
    pub stack_names: BTreeMap<String, String>,
    #[serde(default)]
    pub visual_sections: BTreeMap<String, VisualSection>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveMutation {
    Set(bool),
    Prune,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfigMutation {
    pub color_updates: BTreeMap<BranchId, Option<Arc<str>>>,
    pub archive_updates: BTreeMap<BranchId, ArchiveMutation>,
    pub stack_name_updates: BTreeMap<BranchId, Option<Arc<str>>>,
    pub visual_section_updates: BTreeMap<BranchId, Option<VisualSection>>,
}

impl ConfigMutation {
    pub fn set_color(&mut self, root: BranchId, color: Option<Arc<str>>) {
        self.color_updates.insert(root, color);
    }

    pub fn set_archived(&mut self, branch: BranchId, archived: bool) {
        self.archive_updates
            .insert(branch, ArchiveMutation::Set(archived));
    }

    pub fn set_stack_name(&mut self, stack: BranchId, name: Option<Arc<str>>) {
        self.stack_name_updates.insert(stack, name);
    }

    pub fn set_visual_section(&mut self, anchor: BranchId, section: Option<VisualSection>) {
        self.visual_section_updates.insert(anchor, section);
    }

    pub fn prune_archived(&mut self, branches: impl IntoIterator<Item = BranchId>) {
        self.archive_updates.extend(
            branches
                .into_iter()
                .map(|branch| (branch, ArchiveMutation::Prune)),
        );
    }

    pub fn merge(&mut self, newer: Self) {
        self.color_updates.extend(newer.color_updates);
        self.archive_updates.extend(newer.archive_updates);
        self.stack_name_updates.extend(newer.stack_name_updates);
        self.visual_section_updates
            .extend(newer.visual_section_updates);
    }

    pub fn is_empty(&self) -> bool {
        self.color_updates.is_empty()
            && self.archive_updates.is_empty()
            && self.stack_name_updates.is_empty()
            && self.visual_section_updates.is_empty()
    }
}

impl Config {
    pub fn load(common_dir: &Path) -> Result<Self> {
        let path = config_path(common_dir);
        if !path.exists() {
            return Ok(Self::default());
        }
        let config: Self = toml::from_str(&fs::read_to_string(&path)?)?;
        for color in config.colors.values() {
            validate_color(color)?;
        }
        for name in config.stack_names.values() {
            validate_stack_name(name)?;
        }
        for section in config.visual_sections.values() {
            validate_color(&section.color)?;
            if let Some(name) = &section.name {
                validate_stack_name(name)?;
            }
        }
        Ok(config)
    }

    pub fn color(&self, root: &BranchId) -> Option<&str> {
        self.colors.get(root.0.as_ref()).map(String::as_str)
    }

    pub fn is_archived(&self, branch: &BranchId) -> bool {
        self.archived.contains(branch.0.as_ref())
    }

    pub fn stack_name(&self, stack: &BranchId) -> Option<&str> {
        self.stack_names.get(stack.0.as_ref()).map(String::as_str)
    }

    pub fn visual_section(&self, anchor: &BranchId) -> Option<&VisualSection> {
        self.visual_sections.get(anchor.0.as_ref())
    }

    pub fn set_visual_section_in_memory(
        &mut self,
        anchor: &BranchId,
        section: Option<VisualSection>,
    ) -> Result<()> {
        if let Some(section) = section {
            validate_color(&section.color)?;
            if let Some(name) = &section.name {
                validate_stack_name(name)?;
            }
            self.visual_sections.insert(anchor.0.to_string(), section);
        } else {
            self.visual_sections.remove(anchor.0.as_ref());
        }
        Ok(())
    }

    pub fn set_stack_name_in_memory(&mut self, stack: &BranchId, name: Option<&str>) -> Result<()> {
        let name = name.map(str::trim).filter(|name| !name.is_empty());
        if let Some(name) = name {
            validate_stack_name(name)?;
            self.stack_names
                .insert(stack.0.to_string(), name.to_owned());
        } else {
            self.stack_names.remove(stack.0.as_ref());
        }
        Ok(())
    }

    pub fn set_color(
        &mut self,
        common_dir: &Path,
        root: &BranchId,
        color: Option<&str>,
    ) -> Result<()> {
        let fallback = self.clone();
        Self::persist_color(common_dir, root, color, &fallback)?;
        self.set_color_in_memory(root, color)?;
        Ok(())
    }

    pub fn set_color_in_memory(&mut self, root: &BranchId, color: Option<&str>) -> Result<()> {
        if let Some(color) = color {
            validate_color(color)?;
            self.colors.insert(root.0.to_string(), color.to_owned());
        } else {
            self.colors.remove(root.0.as_ref());
        }
        Ok(())
    }

    pub fn set_archived(
        &mut self,
        common_dir: &Path,
        branch: &BranchId,
        archived: bool,
    ) -> Result<()> {
        let fallback = self.clone();
        Self::persist_archived(common_dir, branch, archived, &fallback)?;
        self.set_archived_in_memory(branch, archived);
        Ok(())
    }

    pub fn set_archived_in_memory(&mut self, branch: &BranchId, archived: bool) {
        if archived {
            self.archived.insert(branch.0.to_string());
        } else {
            self.archived.remove(branch.0.as_ref());
        }
    }

    pub fn persist_color(
        common_dir: &Path,
        root: &BranchId,
        color: Option<&str>,
        fallback: &Self,
    ) -> Result<()> {
        let updates = BTreeMap::from([(root.clone(), color.map(Arc::from))]);
        Self::persist_colors(common_dir, &updates, fallback)
    }

    pub fn persist_colors(
        common_dir: &Path,
        updates: &BTreeMap<BranchId, Option<Arc<str>>>,
        fallback: &Self,
    ) -> Result<()> {
        let mutation = ConfigMutation {
            color_updates: updates.clone(),
            archive_updates: BTreeMap::new(),
            stack_name_updates: BTreeMap::new(),
            visual_section_updates: BTreeMap::new(),
        };
        Self::persist_mutation(common_dir, &mutation, fallback)
    }

    pub fn persist_archived(
        common_dir: &Path,
        branch: &BranchId,
        archived: bool,
        fallback: &Self,
    ) -> Result<()> {
        let mut mutation = ConfigMutation::default();
        mutation.set_archived(branch.clone(), archived);
        Self::persist_mutation(common_dir, &mutation, fallback)
    }

    pub fn persist_mutation(
        common_dir: &Path,
        mutation: &ConfigMutation,
        fallback: &Self,
    ) -> Result<()> {
        let _lock = ConfigLock::acquire(common_dir)?;
        // Merge against the latest on-disk value so two worktrees changing
        // different config fields or branch names do not overwrite one another.
        let mut merged = Self::load(common_dir).unwrap_or_else(|_| fallback.clone());
        merged.apply_mutation_in_memory(mutation)?;
        merged.save(common_dir)?;
        Ok(())
    }

    pub fn apply_mutation_in_memory(&mut self, mutation: &ConfigMutation) -> Result<()> {
        for color in mutation.color_updates.values().flatten() {
            validate_color(color)?;
        }
        for name in mutation.stack_name_updates.values().flatten() {
            validate_stack_name(name)?;
        }
        for section in mutation.visual_section_updates.values().flatten() {
            validate_color(&section.color)?;
            if let Some(name) = &section.name {
                validate_stack_name(name)?;
            }
        }
        for (root, color) in &mutation.color_updates {
            if let Some(color) = color {
                self.colors.insert(root.0.to_string(), color.to_string());
            } else {
                self.colors.remove(root.0.as_ref());
            }
        }
        for (branch, update) in &mutation.archive_updates {
            match update {
                ArchiveMutation::Set(true) => {
                    self.archived.insert(branch.0.to_string());
                }
                ArchiveMutation::Set(false) | ArchiveMutation::Prune => {
                    self.archived.remove(branch.0.as_ref());
                }
            }
        }
        for (stack, name) in &mutation.stack_name_updates {
            self.set_stack_name_in_memory(stack, name.as_deref())?;
        }
        for (anchor, section) in &mutation.visual_section_updates {
            self.set_visual_section_in_memory(anchor, section.clone())?;
        }
        Ok(())
    }

    fn save(&self, common_dir: &Path) -> Result<()> {
        let path = config_path(common_dir);
        let parent = path.parent().context("config parent")?;
        fs::create_dir_all(parent)?;
        let contents = toml::to_string_pretty(self)?;
        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        let mut file = fs::File::create(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(temporary, path)?;
        Ok(())
    }
}

struct ConfigLock {
    path: PathBuf,
}

impl ConfigLock {
    fn acquire(common_dir: &Path) -> Result<Self> {
        let path = config_path(common_dir).with_extension("lock");
        let parent = path.parent().context("config lock parent")?;
        fs::create_dir_all(parent)?;
        let started = Instant::now();
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())?;
                    file.sync_all()?;
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                        .is_some_and(|age| age >= Duration::from_secs(10));
                    if stale {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    if started.elapsed() >= Duration::from_secs(2) {
                        bail!("timed out waiting for the stackmap config lock");
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn config_path(common_dir: &Path) -> PathBuf {
    common_dir.join("stackmap").join("config.toml")
}

fn validate_color(color: &str) -> Result<()> {
    let valid_hex = color.len() == 7
        && color.starts_with('#')
        && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit());
    if !valid_hex {
        bail!("color must use #RRGGBB")
    }
    let normalized = color.to_ascii_lowercase();
    if ["#ff0000", "#00ff00", "#ffff00"].contains(&normalized.as_str()) {
        bail!("red, green, and yellow are reserved semantic colors")
    }
    Ok(())
}

fn validate_stack_name(name: &str) -> Result<()> {
    if name.trim() != name {
        bail!("stack names cannot start or end with whitespace")
    }
    if name.is_empty() {
        bail!("stack names cannot be empty")
    }
    if name.chars().count() > MAX_STACK_NAME_CHARS {
        bail!("stack names are limited to {MAX_STACK_NAME_CHARS} characters")
    }
    if name.chars().any(char::is_control) {
        bail!("stack names cannot contain control characters")
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn concurrent_updates_retain_both_values() {
        let directory = tempdir().unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for (name, color) in [("one", "#7aa2f7"), ("two", "#bb9af7")] {
            let path = directory.path().to_owned();
            let barrier = barrier.clone();
            handles.push(thread::spawn(move || {
                let mut config = Config::default();
                barrier.wait();
                config
                    .set_color(&path, &BranchId::new(name), Some(color))
                    .unwrap();
            }));
        }
        barrier.wait();
        for handle in handles {
            handle.join().unwrap();
        }
        let config = Config::load(directory.path()).unwrap();
        assert_eq!(config.colors.len(), 2);
        assert_eq!(config.colors["one"], "#7aa2f7");
        assert_eq!(config.colors["two"], "#bb9af7");
    }

    #[test]
    fn legacy_toml_without_archived_branches_loads() {
        let directory = tempdir().unwrap();
        let path = config_path(directory.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "[colors]\none = \"#7aa2f7\"\n").unwrap();

        let config = Config::load(directory.path()).unwrap();

        assert_eq!(config.colors["one"], "#7aa2f7");
        assert!(config.archived.is_empty());
    }

    #[test]
    fn concurrent_color_and_archive_mutations_retain_both_fields() {
        let directory = tempdir().unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let color_path = directory.path().to_owned();
        let color_barrier = barrier.clone();
        let color = thread::spawn(move || {
            let mut mutation = ConfigMutation::default();
            mutation.set_color(BranchId::new("colored"), Some(Arc::from("#7aa2f7")));
            color_barrier.wait();
            Config::persist_mutation(&color_path, &mutation, &Config::default()).unwrap();
        });
        let archive_path = directory.path().to_owned();
        let archive_barrier = barrier.clone();
        let archive = thread::spawn(move || {
            let mut mutation = ConfigMutation::default();
            mutation.set_archived(BranchId::new("hidden"), true);
            archive_barrier.wait();
            Config::persist_mutation(&archive_path, &mutation, &Config::default()).unwrap();
        });

        barrier.wait();
        color.join().unwrap();
        archive.join().unwrap();

        let config = Config::load(directory.path()).unwrap();
        assert_eq!(config.colors["colored"], "#7aa2f7");
        assert_eq!(config.archived, BTreeSet::from(["hidden".to_owned()]));
    }

    #[test]
    fn stack_names_round_trip_and_empty_mutation_clears_them() {
        let directory = tempdir().unwrap();
        let stack = BranchId::new("stack-root");
        let mut mutation = ConfigMutation::default();
        mutation.set_stack_name(stack.clone(), Some(Arc::from("Release train")));
        Config::persist_mutation(directory.path(), &mutation, &Config::default()).unwrap();

        let loaded = Config::load(directory.path()).unwrap();
        assert_eq!(loaded.stack_name(&stack), Some("Release train"));

        let mut clear = ConfigMutation::default();
        clear.set_stack_name(stack.clone(), None);
        Config::persist_mutation(directory.path(), &clear, &loaded).unwrap();
        assert_eq!(
            Config::load(directory.path()).unwrap().stack_name(&stack),
            None
        );
    }

    #[test]
    fn stack_names_are_bounded_single_line_text() {
        let mut config = Config::default();
        let stack = BranchId::new("stack-root");
        assert!(
            config
                .set_stack_name_in_memory(&stack, Some("line\nbreak"))
                .is_err()
        );
        assert!(
            config
                .set_stack_name_in_memory(&stack, Some(&"x".repeat(MAX_STACK_NAME_CHARS + 1)))
                .is_err()
        );
        config
            .set_stack_name_in_memory(&stack, Some("  trimmed name  "))
            .unwrap();
        assert_eq!(config.stack_name(&stack), Some("trimmed name"));
    }

    #[test]
    fn visual_sections_round_trip_and_are_removed_atomically() {
        let directory = tempdir().unwrap();
        let anchor = BranchId::new("feature-two");
        let section = VisualSection {
            color: "#7dcfff".to_owned(),
            name: Some("Payments".to_owned()),
        };
        let mut create = ConfigMutation::default();
        create.set_visual_section(anchor.clone(), Some(section.clone()));
        Config::persist_mutation(directory.path(), &create, &Config::default()).unwrap();
        assert_eq!(
            Config::load(directory.path())
                .unwrap()
                .visual_section(&anchor),
            Some(&section)
        );

        let mut remove = ConfigMutation::default();
        remove.set_visual_section(anchor.clone(), None);
        Config::persist_mutation(directory.path(), &remove, &Config::default()).unwrap();
        assert_eq!(
            Config::load(directory.path())
                .unwrap()
                .visual_section(&anchor),
            None
        );
    }

    #[test]
    fn visual_section_names_and_colors_are_validated() {
        let mut config = Config::default();
        let anchor = BranchId::new("feature-two");
        assert!(
            config
                .set_visual_section_in_memory(
                    &anchor,
                    Some(VisualSection {
                        color: "red".to_owned(),
                        name: None,
                    })
                )
                .is_err()
        );
        assert!(
            config
                .set_visual_section_in_memory(
                    &anchor,
                    Some(VisualSection {
                        color: "#7dcfff".to_owned(),
                        name: Some("bad\nname".to_owned()),
                    })
                )
                .is_err()
        );
    }

    #[test]
    fn concurrent_archive_mutations_retain_both_memberships() {
        let directory = tempdir().unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for name in ["one", "two"] {
            let path = directory.path().to_owned();
            let barrier = barrier.clone();
            handles.push(thread::spawn(move || {
                let mut mutation = ConfigMutation::default();
                mutation.set_archived(BranchId::new(name), true);
                barrier.wait();
                Config::persist_mutation(&path, &mutation, &Config::default()).unwrap();
            }));
        }

        barrier.wait();
        for handle in handles {
            handle.join().unwrap();
        }

        let config = Config::load(directory.path()).unwrap();
        assert_eq!(
            config.archived,
            BTreeSet::from(["one".to_owned(), "two".to_owned()])
        );
    }

    #[test]
    fn mutation_prunes_only_named_archives_and_preserves_other_fields() {
        let directory = tempdir().unwrap();
        let fallback = Config {
            colors: BTreeMap::from([("colored".to_owned(), "#7aa2f7".to_owned())]),
            archived: BTreeSet::from(["gone".to_owned(), "kept".to_owned(), "restored".to_owned()]),
            stack_names: BTreeMap::from([("named".to_owned(), "Keep me".to_owned())]),
            visual_sections: BTreeMap::new(),
        };
        fallback.save(directory.path()).unwrap();
        let mut mutation = ConfigMutation::default();
        mutation.prune_archived([BranchId::new("gone")]);
        mutation.set_archived(BranchId::new("restored"), false);
        mutation.set_archived(BranchId::new("added"), true);

        Config::persist_mutation(directory.path(), &mutation, &fallback).unwrap();

        let config = Config::load(directory.path()).unwrap();
        assert_eq!(config.colors, fallback.colors);
        assert_eq!(config.stack_names, fallback.stack_names);
        assert_eq!(
            config.archived,
            BTreeSet::from(["added".to_owned(), "kept".to_owned()])
        );
    }
}
