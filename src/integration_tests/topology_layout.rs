use super::common;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::model::BranchId;
use crate::model::topology::{
    ArchiveMode, DividerRow, Emphasis, OrderMode, ProjectionEntry, ProjectionOptions,
    ProjectionScope, TopologyIndex,
};

fn tracked(
    name: &str,
    parent: Option<&str>,
    root: &str,
    trunk: &str,
    committed_at: i64,
) -> crate::model::Branch {
    let mut branch = common::branch(name, parent, root, false);
    branch.trunk = Some(BranchId::new(trunk));
    branch.graphite = crate::model::GraphiteProvenance::Tracked;
    branch.committed_at = committed_at;
    branch
}

fn branch_names(projection: &crate::model::topology::TopologyProjection) -> Vec<&str> {
    projection
        .entries
        .iter()
        .filter_map(|entry| match entry {
            ProjectionEntry::Branch(row) => Some(row.branch.0.as_ref()),
            _ => None,
        })
        .collect()
}

fn fork_snapshot() -> Arc<crate::model::RepositorySnapshot> {
    let mut snapshot = (*common::snapshot(vec![
        tracked("staging", None, "staging", "staging", 1),
        tracked("1", None, "1", "staging", 2),
        tracked("2", Some("1"), "1", "staging", 3),
        tracked("3", Some("2"), "1", "staging", 4),
        tracked("4", Some("3"), "1", "staging", 5),
        tracked("3b", Some("3"), "1", "staging", 6),
        tracked("3c", Some("3b"), "1", "staging", 7),
    ]))
    .clone();
    snapshot.configured_trunks = Arc::from([BranchId::new("staging")]);
    snapshot.trunks = snapshot.configured_trunks.clone();
    snapshot.graphite_children = Arc::from([
        (BranchId::new("staging"), Arc::from([BranchId::new("1")])),
        (BranchId::new("1"), Arc::from([BranchId::new("2")])),
        (BranchId::new("2"), Arc::from([BranchId::new("3")])),
        (
            BranchId::new("3"),
            Arc::from([BranchId::new("4"), BranchId::new("3b")]),
        ),
        (BranchId::new("3b"), Arc::from([BranchId::new("3c")])),
    ]);
    Arc::new(snapshot)
}

#[test]
fn linear_stack_is_bottom_up_with_trunk_at_section_bottom() {
    let mut snapshot = (*common::snapshot(vec![
        tracked("main", None, "main", "main", 1),
        tracked("root", None, "root", "main", 2),
        tracked("middle", Some("root"), "root", "main", 3),
        tracked("tip", Some("middle"), "root", "main", 4),
    ]))
    .clone();
    snapshot.graphite_children = Arc::from([
        (BranchId::new("main"), Arc::from([BranchId::new("root")])),
        (BranchId::new("root"), Arc::from([BranchId::new("middle")])),
        (BranchId::new("middle"), Arc::from([BranchId::new("tip")])),
    ]);
    let projection = TopologyIndex::build(&snapshot).project(&ProjectionOptions::default());

    assert_eq!(branch_names(&projection), ["tip", "middle", "root", "main"]);
    assert_eq!(projection.stack_heads.len(), 1);
    assert_eq!(projection.stack_heads[0].branch, BranchId::new("tip"));
    assert_eq!(projection.row_for(&BranchId::new("root")).unwrap().lane, 1);
    assert_eq!(projection.row_for(&BranchId::new("main")).unwrap().lane, 0);
}

#[test]
fn stack_name_is_a_nonselectable_row_immediately_above_the_stack_head() {
    let snapshot = fork_snapshot();
    let projection = TopologyIndex::build(&snapshot).project(&ProjectionOptions {
        stack_names: HashMap::from([(BranchId::new("1"), Arc::<str>::from("Primary work"))]),
        ..ProjectionOptions::default()
    });
    let head = projection
        .stack_heads
        .iter()
        .find(|head| head.stack_id == BranchId::new("1"))
        .expect("named stack head");
    let label_row = head.visual_row.checked_sub(1).expect("label before head");
    assert!(matches!(
        &projection.entries[label_row],
        ProjectionEntry::StackLabel(label)
            if label.stack_id == BranchId::new("1") && label.text.as_ref() == "Primary work"
    ));
    assert_eq!(
        projection.selectable.len(),
        projection
            .entries
            .iter()
            .filter(|entry| matches!(entry, ProjectionEntry::Branch(_)))
            .count()
    );
}

#[test]
fn first_child_stays_straight_and_side_stack_attaches_to_exact_parent() {
    let projection = TopologyIndex::build(&fork_snapshot()).project(&ProjectionOptions::default());

    assert_eq!(
        branch_names(&projection),
        ["4", "3c", "3b", "3", "2", "1", "staging"]
    );
    for branch in ["1", "2", "3", "4"] {
        let row = projection.row_for(&BranchId::new(branch)).unwrap();
        assert_eq!(row.stack_id, BranchId::new("1"));
        assert_eq!(row.lane, 1);
    }
    for branch in ["3b", "3c"] {
        let row = projection.row_for(&BranchId::new(branch)).unwrap();
        assert_eq!(row.stack_id, BranchId::new("3b"));
        assert_eq!(row.lane, 2);
    }
    let connector = projection
        .entries
        .iter()
        .find_map(|entry| match entry {
            ProjectionEntry::Divider(DividerRow::Connector(connector))
                if connector.stack_id == BranchId::new("3b") =>
            {
                Some(connector)
            }
            _ => None,
        })
        .expect("side-stack connector");
    assert_eq!(connector.parent.as_ref(), Some(&BranchId::new("3")));
    assert_eq!((connector.from_lane, connector.to_lane), (2, 1));
}

#[test]
fn roots_connect_to_exclusive_trunk_anchor_and_spacers_are_optional() {
    let index = TopologyIndex::build(&fork_snapshot());
    let spaced = index.project(&ProjectionOptions::default());
    let root_connector = spaced
        .entries
        .iter()
        .find_map(|entry| match entry {
            ProjectionEntry::Divider(DividerRow::Connector(connector))
                if connector.stack_id == BranchId::new("1") =>
            {
                Some(connector)
            }
            _ => None,
        })
        .expect("root connector");
    assert_eq!(
        root_connector.parent.as_ref(),
        Some(&BranchId::new("staging"))
    );
    assert_eq!((root_connector.from_lane, root_connector.to_lane), (1, 0));
    assert!(
        spaced
            .entries
            .iter()
            .any(|entry| matches!(entry, ProjectionEntry::Divider(DividerRow::Spacer { .. })))
    );

    let compact = index.project(&ProjectionOptions {
        separators: false,
        ..ProjectionOptions::default()
    });
    assert!(
        !compact
            .entries
            .iter()
            .any(|entry| matches!(entry, ProjectionEntry::Divider(DividerRow::Spacer { .. })))
    );
    assert!(
        compact
            .entries
            .iter()
            .any(|entry| matches!(entry, ProjectionEntry::Divider(DividerRow::Connector(_))))
    );
}

#[test]
fn non_overlapping_sibling_stacks_reuse_hierarchical_lane() {
    let mut snapshot = (*fork_snapshot()).clone();
    let mut branches = snapshot.branches.to_vec();
    branches.push(tracked("2b", Some("2"), "1", "staging", 8));
    snapshot.branch_index = crate::model::RepositorySnapshot::index_branches(&branches);
    snapshot.branches = Arc::from(branches);
    snapshot.graphite_children = Arc::from([
        (
            BranchId::new("2"),
            Arc::from([BranchId::new("3"), BranchId::new("2b")]),
        ),
        (
            BranchId::new("3"),
            Arc::from([BranchId::new("4"), BranchId::new("3b")]),
        ),
        (BranchId::new("3b"), Arc::from([BranchId::new("3c")])),
    ]);
    let projection = TopologyIndex::build(&snapshot).project(&ProjectionOptions::default());
    assert_eq!(projection.row_for(&BranchId::new("2b")).unwrap().lane, 2);
    assert_eq!(projection.row_for(&BranchId::new("3b")).unwrap().lane, 2);
    let spans: Vec<_> = projection
        .lane_spans
        .iter()
        .filter(|span| span.lane == 2)
        .collect();
    assert!(spans.windows(2).all(|pair| pair[0].end < pair[1].start));
}

#[test]
fn multiple_trunks_are_ordered_and_untrunked_is_last() {
    let mut snapshot = (*common::snapshot(vec![
        tracked("main", None, "main", "main", 1),
        tracked("main-feature", None, "main-feature", "main", 2),
        tracked("preview", None, "preview", "preview", 1),
        tracked("preview-feature", None, "preview-feature", "preview", 2),
        common::branch("loose", None, "loose", false),
    ]))
    .clone();
    snapshot.configured_trunks = Arc::from([BranchId::new("main"), BranchId::new("preview")]);
    snapshot.trunks = snapshot.configured_trunks.clone();
    let projection = TopologyIndex::build(&snapshot).project(&ProjectionOptions::default());
    let sections: Vec<_> = projection
        .entries
        .iter()
        .filter_map(|entry| match entry {
            ProjectionEntry::Section(section) => Some(section.title.as_ref()),
            _ => None,
        })
        .collect();

    assert_eq!(sections, ["main", "preview", "Untrunked"]);
    assert_eq!(branch_names(&projection).last(), Some(&"loose"));
    assert_eq!(branch_names(&projection).len(), 5);
    assert_eq!(projection.section_ranges.len(), 3);
    for range in &projection.section_ranges {
        assert!(range.start <= range.bottom);
        assert!(range.bottom <= range.end);
    }
}

#[test]
fn order_modes_keep_one_offs_above_complete_stacks_and_break_ties_stably() {
    let mut snapshot = (*common::snapshot(vec![
        tracked("main", None, "main", "main", 1),
        tracked("z-new-one-off", None, "z-new-one-off", "main", 200),
        tracked("a-old-one-off", None, "a-old-one-off", "main", 10),
        tracked("beta", None, "beta", "main", 20),
        tracked("beta-tip", Some("beta"), "beta", "main", 20),
        tracked("alpha", None, "alpha", "main", 200),
        tracked("alpha-tip", Some("alpha"), "alpha", "main", 200),
    ]))
    .clone();
    snapshot.graphite_children = Arc::from([(
        BranchId::new("main"),
        Arc::from([
            BranchId::new("z-new-one-off"),
            BranchId::new("a-old-one-off"),
            BranchId::new("alpha"),
            BranchId::new("beta"),
        ]),
    )]);
    let index = TopologyIndex::build(&snapshot);
    let recent = index.project(&ProjectionOptions::default());
    assert_eq!(
        branch_names(&recent),
        [
            "a-old-one-off",
            "z-new-one-off",
            "beta-tip",
            "beta",
            "alpha-tip",
            "alpha",
            "main"
        ]
    );

    let alphabetical = index.project(&ProjectionOptions {
        order: OrderMode::Alphabetical,
        ..ProjectionOptions::default()
    });
    assert_eq!(
        branch_names(&alphabetical),
        [
            "a-old-one-off",
            "z-new-one-off",
            "alpha-tip",
            "alpha",
            "beta-tip",
            "beta",
            "main"
        ]
    );

    let graphite = index.project(&ProjectionOptions {
        order: OrderMode::Graphite,
        ..ProjectionOptions::default()
    });
    assert_eq!(
        branch_names(&graphite),
        [
            "z-new-one-off",
            "a-old-one-off",
            "alpha-tip",
            "alpha",
            "beta-tip",
            "beta",
            "main"
        ]
    );
}

#[test]
fn stack_scope_keeps_ancestor_path_full_dims_siblings_and_hides_other_trunks() {
    let snapshot = common::snapshot(vec![
        tracked("main", None, "main", "main", 1),
        tracked("alpha", None, "alpha", "main", 2),
        tracked("alpha-tip", Some("alpha"), "alpha", "main", 3),
        tracked("beta", None, "beta", "main", 4),
        tracked("preview", None, "preview", "preview", 1),
        tracked("preview-work", None, "preview-work", "preview", 2),
    ]);
    let index = TopologyIndex::build(&snapshot);
    let all = index.project(&ProjectionOptions::default());
    assert!(
        all.entries
            .iter()
            .any(|entry| matches!(entry, ProjectionEntry::Divider(DividerRow::Spacer { .. })))
    );

    let scoped = index.project(&ProjectionOptions {
        scope: ProjectionScope::Stack(BranchId::new("alpha-tip")),
        ..ProjectionOptions::default()
    });
    assert_eq!(
        branch_names(&scoped),
        ["beta", "alpha-tip", "alpha", "main"]
    );
    for branch in ["alpha-tip", "alpha", "main"] {
        assert_eq!(scoped.emphasis_for(&BranchId::new(branch)), Emphasis::Full);
    }
    assert_eq!(scoped.emphasis_for(&BranchId::new("beta")), Emphasis::Dim);
    assert_eq!(
        scoped.emphasis_for(&BranchId::new("preview-work")),
        Emphasis::Hidden
    );
}

#[test]
fn side_stack_scope_keeps_shared_ancestry_full_and_dims_primary_sibling() {
    let mut snapshot = (*common::snapshot(vec![
        tracked("main", None, "main", "main", 1),
        tracked("root", None, "root", "main", 2),
        tracked("primary", Some("root"), "root", "main", 3),
        tracked("side", Some("root"), "root", "main", 4),
        tracked("side-tip", Some("side"), "root", "main", 5),
    ]))
    .clone();
    snapshot.graphite_children = Arc::from([(
        BranchId::new("root"),
        Arc::from([BranchId::new("primary"), BranchId::new("side")]),
    )]);
    let projection = TopologyIndex::build(&snapshot).project(&ProjectionOptions {
        scope: ProjectionScope::Stack(BranchId::new("side-tip")),
        ..ProjectionOptions::default()
    });
    assert_eq!(
        branch_names(&projection),
        ["primary", "side-tip", "side", "root", "main"]
    );
    for branch in ["side-tip", "side", "root", "main"] {
        assert_eq!(
            projection.emphasis_for(&BranchId::new(branch)),
            Emphasis::Full
        );
    }
    assert_eq!(
        projection.emphasis_for(&BranchId::new("primary")),
        Emphasis::Dim
    );
}

#[test]
fn trunk_scope_keeps_only_its_section_and_untrunked_stack_scope_isolated() {
    let mut snapshot = (*common::snapshot(vec![
        tracked("main", None, "main", "main", 1),
        tracked("main-work", None, "main-work", "main", 2),
        tracked("preview", None, "preview", "preview", 1),
        tracked("preview-work", None, "preview-work", "preview", 2),
        common::branch("loose-a", None, "loose-a", false),
        common::branch("loose-a-primary", Some("loose-a"), "loose-a", false),
        common::branch("loose-a-side", Some("loose-a"), "loose-a", false),
        common::branch("loose-b", None, "loose-b", false),
    ]))
    .clone();
    snapshot.configured_trunks = Arc::from([BranchId::new("main"), BranchId::new("preview")]);
    snapshot.trunks = snapshot.configured_trunks.clone();
    let index = TopologyIndex::build(&snapshot);

    let trunk = index.project(&ProjectionOptions {
        scope: ProjectionScope::Trunk(BranchId::new("preview")),
        ..ProjectionOptions::default()
    });
    assert_eq!(branch_names(&trunk), ["preview-work", "preview"]);
    assert_eq!(
        trunk.emphasis_for(&BranchId::new("main-work")),
        Emphasis::Hidden
    );

    let loose = index.project(&ProjectionOptions {
        scope: ProjectionScope::Stack(BranchId::new("loose-a-primary")),
        ..ProjectionOptions::default()
    });
    assert_eq!(
        branch_names(&loose),
        ["loose-a-primary", "loose-a-side", "loose-a"]
    );
    assert_eq!(
        loose.emphasis_for(&BranchId::new("loose-a-primary")),
        Emphasis::Full
    );
    assert_eq!(
        loose.emphasis_for(&BranchId::new("loose-a-side")),
        Emphasis::Dim
    );
    assert_eq!(
        loose.emphasis_for(&BranchId::new("loose-b")),
        Emphasis::Hidden
    );
}

#[test]
fn recent_equal_activity_uses_graphite_position_for_roots_and_siblings() {
    let mut roots = (*common::snapshot(vec![
        tracked("main", None, "main", "main", 1),
        tracked("z-root", None, "z-root", "main", 10),
        tracked("a-root", None, "a-root", "main", 10),
    ]))
    .clone();
    roots.graphite_children = Arc::from([(
        BranchId::new("main"),
        Arc::from([BranchId::new("z-root"), BranchId::new("a-root")]),
    )]);
    let projection = TopologyIndex::build(&roots).project(&ProjectionOptions::default());
    assert_eq!(branch_names(&projection), ["z-root", "a-root", "main"]);

    let mut siblings = (*common::snapshot(vec![
        tracked("main", None, "main", "main", 1),
        tracked("root", None, "root", "main", 2),
        tracked("primary", Some("root"), "root", "main", 3),
        tracked("z-side", Some("root"), "root", "main", 10),
        tracked("a-side", Some("root"), "root", "main", 10),
    ]))
    .clone();
    siblings.graphite_children = Arc::from([(
        BranchId::new("root"),
        Arc::from([
            BranchId::new("primary"),
            BranchId::new("z-side"),
            BranchId::new("a-side"),
        ]),
    )]);
    let projection = TopologyIndex::build(&siblings).project(&ProjectionOptions::default());
    assert_eq!(
        branch_names(&projection),
        ["primary", "z-side", "a-side", "root", "main"]
    );
}

#[test]
fn filtered_ancestry_worklist_terminates_on_a_malformed_parent_cycle() {
    let snapshot = common::snapshot(vec![
        common::branch("cycle-a", Some("cycle-b"), "cycle-a", false),
        common::branch("cycle-b", Some("cycle-a"), "cycle-a", false),
    ]);
    let projection = TopologyIndex::build(&snapshot).project(&ProjectionOptions {
        filter: "cycle-a".into(),
        ..ProjectionOptions::default()
    });
    assert!(projection.selectable.is_empty());
    assert!(projection.entries.is_empty());
}

#[test]
fn archived_internal_branches_become_nonselectable_continuation_placeholders() {
    let snapshot = common::snapshot(vec![
        tracked("main", None, "main", "main", 1),
        tracked("root", None, "root", "main", 2),
        tracked("middle", Some("root"), "root", "main", 3),
        tracked("tip", Some("middle"), "root", "main", 4),
    ]);
    let index = TopologyIndex::build(&snapshot);
    let archived = HashSet::from([BranchId::new("middle")]);
    let active = index.project(&ProjectionOptions {
        archived: archived.clone(),
        ..ProjectionOptions::default()
    });
    assert_eq!(branch_names(&active), ["tip", "root", "main"]);
    assert!(!active.selectable.contains(&BranchId::new("middle")));
    assert!(active.entries.iter().any(|entry| matches!(
        entry,
        ProjectionEntry::Divider(DividerRow::Placeholder(row))
            if row.branch == BranchId::new("middle")
                && row.parent.as_ref() == Some(&BranchId::new("root"))
    )));
    assert!(active.is_true_stack(&BranchId::new("root")));

    let archive = index.project(&ProjectionOptions {
        archive_mode: ArchiveMode::Archive,
        archived,
        ..ProjectionOptions::default()
    });
    assert_eq!(branch_names(&archive), ["middle", "main"]);
    assert!(archive.entries.iter().any(|entry| matches!(
        entry,
        ProjectionEntry::Divider(DividerRow::Placeholder(row))
            if row.branch == BranchId::new("root")
    )));
    assert!(!archive.entries.iter().any(|entry| matches!(
        entry,
        ProjectionEntry::Divider(DividerRow::Placeholder(row))
            if row.branch == BranchId::new("tip")
    )));
}

#[test]
fn five_thousand_branches_have_linear_projection_metadata() {
    for count in [500, 5_000] {
        let mut branches = Vec::with_capacity(count);
        for index in 0..count {
            let root = index - index % 10;
            let name = format!("branch-{index:04}");
            let parent = (index % 10 != 0).then(|| format!("branch-{:04}", index - 1));
            branches.push(common::branch(
                &name,
                parent.as_deref(),
                &format!("branch-{root:04}"),
                false,
            ));
        }
        let snapshot = common::snapshot(branches);
        let projection = TopologyIndex::build(&snapshot).project(&ProjectionOptions::default());
        assert_eq!(projection.selectable.len(), count);
        assert!(projection.entries.len() <= count * 3 + 1);
        assert!(projection.lane_spans.len() <= count);
        assert!(projection.section_ranges.len() <= count + 1);
    }
}

#[test]
fn broad_comb_projects_each_attach_parent_without_quadratic_child_scans() {
    const DEPTH: usize = 2_000;
    let mut branches = Vec::with_capacity(DEPTH * 2 + 2);
    branches.push(tracked("main", None, "main", "main", 0));
    let mut child_order = Vec::with_capacity(DEPTH + 1);
    child_order.push((
        BranchId::new("main"),
        Arc::from([BranchId::new("primary-0000")]),
    ));
    for index in 0..=DEPTH {
        let primary = format!("primary-{index:04}");
        let parent = (index > 0).then(|| format!("primary-{:04}", index - 1));
        branches.push(tracked(
            &primary,
            parent.as_deref(),
            "primary-0000",
            "main",
            index as i64 + 1,
        ));
        if index < DEPTH {
            let side = format!("side-{index:04}");
            branches.push(tracked(
                &side,
                Some(&primary),
                "primary-0000",
                "main",
                index as i64 + 1,
            ));
            let mut children = Vec::with_capacity(2);
            children.push(BranchId::new(format!("primary-{:04}", index + 1)));
            children.push(BranchId::new(side));
            child_order.push((BranchId::new(primary), children.into()));
        }
    }
    let mut snapshot = (*common::snapshot(branches)).clone();
    snapshot.graphite_children = child_order.into();

    let projection = TopologyIndex::build(&snapshot).project(&ProjectionOptions::default());

    assert_eq!(projection.selectable.len(), DEPTH * 2 + 2);
    assert_eq!(
        projection
            .row_for(&BranchId::new("primary-2000"))
            .unwrap()
            .lane,
        1
    );
    for index in [0, DEPTH / 2, DEPTH - 1] {
        let side = BranchId::new(format!("side-{index:04}"));
        let row = projection.row_for(&side).unwrap();
        assert_eq!(row.lane, 2);
        assert_eq!(row.stack_id, side);
    }
    assert!(projection.entries.len() <= DEPTH * 6 + 4);
}

#[test]
fn five_thousand_deep_stack_has_linear_projection_metadata() {
    let mut branches = Vec::with_capacity(5_000);
    for index in 0..5_000 {
        let name = format!("deep-{index:04}");
        let parent = (index > 0).then(|| format!("deep-{:04}", index - 1));
        branches.push(common::branch(&name, parent.as_deref(), "deep-0000", false));
    }

    let snapshot = common::snapshot(branches);
    let projection = TopologyIndex::build(&snapshot).project(&ProjectionOptions::default());

    assert_eq!(projection.selectable.len(), 5_000);
    assert_eq!(projection.entries.len(), 5_001);
    assert_eq!(projection.lane_spans.len(), 1);
    assert_eq!(projection.section_ranges.len(), 1);
    assert_eq!(projection.lane_count, 2);
    assert!(projection.is_true_stack(&BranchId::new("deep-0000")));
}

#[test]
fn five_thousand_deep_comb_emits_every_branch_once_without_recursion() {
    const DEPTH: usize = 5_000;

    let mut branches = Vec::with_capacity(DEPTH * 2);
    for index in 0..DEPTH {
        let side = format!("comb-side-{index:04}");
        let parent = (index > 0).then(|| format!("comb-side-{:04}", index - 1));
        branches.push(common::branch(
            &side,
            parent.as_deref(),
            "comb-side-0000",
            false,
        ));
        branches.push(common::branch(
            &format!("comb-primary-{index:04}"),
            Some(&side),
            "comb-side-0000",
            false,
        ));
    }

    let snapshot = common::snapshot(branches);
    let projection = TopologyIndex::build(&snapshot).project(&ProjectionOptions {
        separators: false,
        ..ProjectionOptions::default()
    });
    let unique: HashSet<_> = projection.selectable.iter().collect();
    let connectors = projection
        .entries
        .iter()
        .filter(|entry| matches!(entry, ProjectionEntry::Divider(DividerRow::Connector(_))))
        .count();

    assert_eq!(projection.selectable.len(), DEPTH * 2);
    assert_eq!(unique.len(), DEPTH * 2);
    assert_eq!(projection.lane_spans.len(), DEPTH);
    assert_eq!(connectors, DEPTH - 1);
    assert_eq!(projection.lane_count, DEPTH + 1);
}
