use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::model::BranchId;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Config {
    #[serde(default)]
    pub colors: BTreeMap<String, String>,
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
        Ok(config)
    }

    pub fn color(&self, root: &BranchId) -> Option<&str> {
        self.colors.get(root.0.as_ref()).map(String::as_str)
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
        let _lock = ConfigLock::acquire(common_dir)?;
        // Merge against the latest on-disk value so two worktrees changing
        // different stack roots do not overwrite one another.
        let mut merged = Self::load(common_dir).unwrap_or_else(|_| fallback.clone());
        for (root, color) in updates {
            if let Some(color) = color {
                validate_color(color)?;
                merged.colors.insert(root.0.to_string(), color.to_string());
            } else {
                merged.colors.remove(root.0.as_ref());
            }
        }
        merged.save(common_dir)?;
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
}
