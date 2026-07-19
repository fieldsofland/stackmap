use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::model::{BranchId, GraphiteProvenance};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GraphiteTopology {
    pub default_trunk: Option<BranchId>,
    pub configured_trunks: Arc<[BranchId]>,
    pub trunks: Arc<[BranchId]>,
    pub parents: HashMap<BranchId, BranchId>,
    pub trunk_by_branch: HashMap<BranchId, BranchId>,
    pub child_order: HashMap<BranchId, Arc<[BranchId]>>,
    pub tracked: HashSet<BranchId>,
    pub degraded: HashSet<BranchId>,
    pub provenance_unknown: bool,
    pub status: Arc<str>,
}

impl GraphiteTopology {
    pub fn provenance(&self, branch: &BranchId) -> GraphiteProvenance {
        if self.degraded.contains(branch)
            || (self.provenance_unknown && !self.tracked.contains(branch))
        {
            GraphiteProvenance::Degraded
        } else if self.tracked.contains(branch) {
            GraphiteProvenance::Tracked
        } else {
            GraphiteProvenance::DefinitelyUntracked
        }
    }
}

pub fn read_topology(common_dir: &Path, local: &HashSet<BranchId>) -> GraphiteTopology {
    let config_path = common_dir.join(".graphite_repo_config");
    let database_path = common_dir.join(".graphite_metadata.db");
    let metadata_present = config_path.exists() || database_path.exists();
    match try_read_topology(common_dir, local) {
        Ok(topology) => topology,
        Err(error) => GraphiteTopology {
            provenance_unknown: metadata_present,
            status: Arc::from(format!("topology unavailable: {error}")),
            ..GraphiteTopology::default()
        },
    }
}

fn try_read_topology(
    common_dir: &Path,
    local: &HashSet<BranchId>,
) -> anyhow::Result<GraphiteTopology> {
    let config_path = common_dir.join(".graphite_repo_config");
    let database_path = common_dir.join(".graphite_metadata.db");
    if !config_path.exists() || !database_path.exists() {
        anyhow::bail!("Graphite metadata not found");
    }
    let config: Value = serde_json::from_slice(&fs::read(config_path)?)?;
    let (default_trunk, configured_trunks) = parse_trunks(&config)?;
    let configured_set: HashSet<_> = configured_trunks.iter().cloned().collect();
    let trunks: Vec<_> = configured_trunks
        .iter()
        .filter(|trunk| local.contains(*trunk))
        .cloned()
        .collect();
    let missing: Vec<_> = configured_trunks
        .iter()
        .filter(|trunk| !local.contains(*trunk))
        .map(ToString::to_string)
        .collect();

    let connection = Connection::open_with_flags(
        database_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let columns = table_columns(&connection, "branch_metadata")?;
    if !columns.contains("branch_name") || !columns.contains("parent_branch_name") {
        anyhow::bail!("unsupported Graphite branch_metadata schema");
    }
    let has_children = columns.contains("children");
    let query = if has_children {
        "SELECT branch_name, parent_branch_name, children FROM branch_metadata"
    } else {
        "SELECT branch_name, parent_branch_name, NULL FROM branch_metadata"
    };
    let mut statement = connection.prepare(query)?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;

    let mut raw_parents = HashMap::new();
    let mut tracked = HashSet::new();
    let mut child_order = HashMap::new();
    for row in rows {
        let (branch_name, parent_name, children) = row?;
        let branch = BranchId::new(branch_name);
        if !local.contains(&branch) {
            continue;
        }
        tracked.insert(branch.clone());
        if let Some(parent_name) = parent_name {
            let parent = BranchId::new(parent_name);
            if raw_parents.insert(branch.clone(), parent).is_some() {
                anyhow::bail!("duplicate parent metadata for {branch}");
            }
        }
        if let Some(children) = children.and_then(|value| parse_children(&value)) {
            child_order.insert(branch, children);
        }
    }
    for trunk in &configured_trunks {
        if local.contains(trunk) {
            tracked.insert(trunk.clone());
        }
    }

    let mut parents = HashMap::new();
    let mut trunk_by_branch = HashMap::new();
    let mut degraded = HashSet::new();
    for branch in &tracked {
        match resolve_trunk(branch, &raw_parents, &configured_set, local) {
            Some(trunk) if local.contains(&trunk) => {
                trunk_by_branch.insert(branch.clone(), trunk);
                if let Some(parent) = raw_parents.get(branch) {
                    parents.insert(branch.clone(), parent.clone());
                }
            }
            _ => {
                degraded.insert(branch.clone());
            }
        }
    }

    let status = if missing.is_empty() {
        format!("Graphite topology loaded ({} trunks)", trunks.len())
    } else {
        format!(
            "Graphite topology loaded; missing configured trunk refs: {}",
            missing.join(", ")
        )
    };
    Ok(GraphiteTopology {
        default_trunk,
        configured_trunks: configured_trunks.into(),
        trunks: trunks.into(),
        parents,
        trunk_by_branch,
        child_order,
        tracked,
        degraded,
        provenance_unknown: false,
        status: Arc::from(status),
    })
}

fn parse_trunks(value: &Value) -> anyhow::Result<(Option<BranchId>, Vec<BranchId>)> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Graphite config is not an object"))?;
    let default = ["trunk", "trunkName", "trunk_name"]
        .into_iter()
        .find_map(|key| object.get(key).and_then(Value::as_str))
        .map(|name| BranchId::new(name.to_owned()));
    let mut trunks = Vec::new();
    if let Some(values) = object.get("trunks").and_then(Value::as_array) {
        for value in values {
            let Some(name) = value
                .as_object()
                .and_then(|entry| entry.get("name"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let trunk = BranchId::new(name.to_owned());
            if !trunks.contains(&trunk) {
                trunks.push(trunk);
            }
        }
    }
    if let Some(default) = &default
        && !trunks.contains(default)
    {
        trunks.push(default.clone());
    }
    if trunks.is_empty() {
        anyhow::bail!("trunk is missing from Graphite config");
    }
    Ok((default, trunks))
}

pub(super) fn raw_branch_metadata_presence(
    common_dir: &Path,
    branch: &BranchId,
) -> anyhow::Result<bool> {
    let database_path = common_dir.join(".graphite_metadata.db");
    if !database_path.exists() {
        anyhow::bail!("Graphite metadata database is missing after deletion");
    }
    let connection = Connection::open_with_flags(
        database_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let columns = table_columns(&connection, "branch_metadata")?;
    if !columns.contains("branch_name") {
        anyhow::bail!("unsupported Graphite branch_metadata schema after deletion");
    }
    let present = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM branch_metadata WHERE branch_name = ?1)",
        [branch.0.as_ref()],
        |row| row.get::<_, bool>(0),
    )?;
    Ok(present)
}

pub(super) fn raw_branch_metadata_has_child(
    common_dir: &Path,
    branch: &BranchId,
) -> anyhow::Result<bool> {
    let database_path = common_dir.join(".graphite_metadata.db");
    if !database_path.exists() {
        anyhow::bail!("Graphite metadata database is missing before deletion");
    }
    let connection = Connection::open_with_flags(
        database_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let columns = table_columns(&connection, "branch_metadata")?;
    if !columns.contains("branch_name") || !columns.contains("parent_branch_name") {
        anyhow::bail!("unsupported Graphite branch_metadata schema before deletion");
    }
    let has_child = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM branch_metadata WHERE parent_branch_name = ?1)",
        [branch.0.as_ref()],
        |row| row.get::<_, bool>(0),
    )?;
    Ok(has_child)
}

fn parse_children(value: &str) -> Option<Arc<[BranchId]>> {
    let values: Vec<String> = serde_json::from_str(value).ok()?;
    let mut children = Vec::with_capacity(values.len());
    for value in values {
        let child = BranchId::new(value);
        if !children.contains(&child) {
            children.push(child);
        }
    }
    Some(children.into())
}

fn resolve_trunk(
    branch: &BranchId,
    parents: &HashMap<BranchId, BranchId>,
    configured: &HashSet<BranchId>,
    local: &HashSet<BranchId>,
) -> Option<BranchId> {
    let mut cursor = branch;
    let mut seen = HashSet::new();
    loop {
        if configured.contains(cursor) {
            return Some(cursor.clone());
        }
        if !local.contains(cursor) || !seen.insert(cursor.clone()) {
            return None;
        }
        cursor = parents.get(cursor)?;
    }
}

fn table_columns(connection: &Connection, table: &str) -> rusqlite::Result<HashSet<String>> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_trunk_order_is_preserved_and_default_is_appended_once() {
        let config = serde_json::json!({
            "trunk": "main",
            "trunks": [
                { "name": "preview" },
                { "name": "main" },
                { "name": "preview" }
            ]
        });
        let (default, trunks) = parse_trunks(&config).unwrap();
        assert_eq!(default, Some(BranchId::new("main")));
        assert_eq!(trunks, [BranchId::new("preview"), BranchId::new("main")]);

        let config = serde_json::json!({
            "trunk": "main",
            "trunks": [{ "name": "preview" }]
        });
        let (_, trunks) = parse_trunks(&config).unwrap();
        assert_eq!(trunks, [BranchId::new("preview"), BranchId::new("main")]);
    }
}
