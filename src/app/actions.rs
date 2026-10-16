use super::*;

pub struct ActionMenuEntry {
    pub group: &'static str,
    pub label: &'static str,
    pub shortcut: &'static str,
    pub key: Key,
}

impl App {
    pub fn action_menu_entries(&self) -> Vec<ActionMenuEntry> {
        let archived = matches!(self.archive_mode, ArchiveMode::Archive);
        let mut entries = Vec::new();
        let mut add = |group, label, shortcut, key| {
            entries.push(ActionMenuEntry {
                group,
                label,
                shortcut,
                key,
            });
        };
        add(
            "Views",
            if archived {
                "Show active branches"
            } else {
                "Show archived branches"
            },
            "a",
            Key::Character('a'),
        );
        add("Views", "Filter branch names", "/", Key::Character('/'));
        add(
            "Views",
            "Refresh repository and selected PR",
            "R",
            Key::Character('R'),
        );
        if self.selected.is_some() {
            add(
                "Branch",
                "Inspect branch and PR status (merged, closed, approved)",
                "d",
                Key::Character('d'),
            );
            add(
                "Branch",
                "Refresh selected pull request",
                "p",
                Key::Character('p'),
            );
            add("Branch", "Copy branch name", "Cmd+C", Key::CopyBranch);
            add(
                "Branch",
                "Copy section branch names",
                "Cmd+Shift+C",
                Key::CopySection,
            );
            add(
                "Branch",
                "Copy stack branch names",
                "Cmd+Opt+Shift+C",
                Key::CopyStack,
            );
            add(
                "Branch",
                "Edit section or stack name",
                "n",
                Key::Character('n'),
            );
            add(
                "Branch",
                "Choose section or stack color",
                "C",
                Key::Character('C'),
            );
        }
        if self.selected_branch().is_some() {
            add(
                "Branch",
                if archived {
                    "Restore branch to Active"
                } else {
                    "Archive branch (keep Git branch)"
                },
                "x",
                Key::Character('x'),
            );
            add(
                "Branch",
                "Select archive / restore range",
                "v",
                Key::Character('v'),
            );
            add(
                "Branch",
                "Switch branch (confirmation)",
                "Enter",
                Key::Enter,
            );
            add(
                "Pull requests",
                "Open selected pull request",
                "o",
                Key::Character('o'),
            );
            add(
                "Pull requests",
                "Open stack pull requests",
                "O",
                Key::Character('O'),
            );
            add(
                "Pull requests",
                "Copy pull request URL",
                "y",
                Key::Character('y'),
            );
            add(
                "Graphite",
                "Restack branch (confirmation)",
                "r",
                Key::Character('r'),
            );
            add("Graphite", "Preview branch move", "m", Key::Character('m'));
            add(
                "Danger",
                "Delete local Git branch (confirmation)",
                "X",
                Key::Character('X'),
            );
        }
        if self.archive_undo.is_some() {
            add(
                "Branch",
                "Undo last archive / restore",
                "u",
                Key::Character('u'),
            );
        }
        add("Views", "Focus selected stack", "h", Key::Character('h'));
        add("Views", "Focus selected trunk", "H", Key::Character('H'));
        add("Views", "Choose branch order", "T", Key::Character('T'));
        add(
            "Appearance",
            "Show / hide branch status",
            "s",
            Key::Character('s'),
        );
        add(
            "Appearance",
            "Show / hide stack spacing",
            "S",
            Key::Character('S'),
        );
        add(
            "Appearance",
            "Increase lane spacing",
            "+",
            Key::Character('+'),
        );
        add(
            "Appearance",
            "Decrease lane spacing",
            "-",
            Key::Character('-'),
        );
        add("Appearance", "Reset lane spacing", "0", Key::Character('0'));
        add(
            "Appearance",
            "Toggle visual section boundary",
            "i",
            Key::Character('i'),
        );
        add("Navigation", "Previous branch", "↑ / k", Key::Up);
        add("Navigation", "Next branch", "↓ / j", Key::Down);
        add(
            "Navigation",
            "Previous stack or ten rows",
            "K / Shift+↑",
            Key::StackUp,
        );
        add(
            "Navigation",
            "Next stack or ten rows",
            "J / Shift+↓",
            Key::StackDown,
        );
        add("Navigation", "Top of section", "g", Key::SectionUp);
        add("Navigation", "Bottom of section", "G", Key::SectionDown);
        add("Application", "Quit Stackmap", "q", Key::Character('q'));
        let query = self.action_query.to_lowercase();
        entries.retain(|entry| {
            let text = format!("{} {} {}", entry.group, entry.label, entry.shortcut).to_lowercase();
            query.split_whitespace().all(|word| text.contains(word))
        });
        entries
    }

    pub(super) fn handle_actions_key(&mut self, input: Input) -> Action {
        if input.phase == KeyPhase::Repeat
            && !matches!(
                input.key,
                Key::Character(_) | Key::Up | Key::Down | Key::Backspace
            )
        {
            return Action::None;
        }
        match input.key {
            Key::Escape => self.overlay = Overlay::None,
            Key::Up => self.action_index = self.action_index.saturating_sub(1),
            Key::Down => {
                self.action_index =
                    (self.action_index + 1).min(self.action_menu_entries().len().saturating_sub(1))
            }
            Key::Home => self.action_index = 0,
            Key::End => self.action_index = self.action_menu_entries().len().saturating_sub(1),
            Key::Backspace => {
                self.action_query.pop();
                self.action_index = 0;
            }
            Key::ClearNameDraft => {
                self.action_query.clear();
                self.action_index = 0;
            }
            Key::Character(ch) if !ch.is_control() => {
                if self.action_query.chars().count() < 80 {
                    self.action_query.push(ch);
                }
                self.action_index = 0;
            }
            Key::Enter => {
                if let Some(entry) = self.action_menu_entries().get(self.action_index) {
                    let key = entry.key.clone();
                    self.overlay = Overlay::None;
                    return self.handle_normal_key(key);
                }
            }
            _ => {}
        }
        Action::None
    }

    pub(super) fn undo_archive(&mut self) -> Action {
        let Some(previous) = self.archive_undo.clone() else {
            self.message = Some(Arc::from("Nothing to undo"));
            return Action::None;
        };
        let Some(snapshot) = self.snapshot.as_ref() else {
            return Action::None;
        };
        if previous.iter().any(|(id, archived)| {
            snapshot.branch(id).is_none()
                || self.config.is_archived(id) == *archived
                || (*archived && self.archive_refusal(id).is_some())
        }) {
            self.message = Some(Arc::from("Archive state changed; undo cancelled"));
            return Action::None;
        }
        let common_dir = snapshot.common_dir.clone();
        let mut mutation = ConfigMutation::default();
        for (id, archived) in &previous {
            self.config.set_archived_in_memory(id, *archived);
            mutation.set_archived(id.clone(), *archived);
        }
        self.archive_undo = None;
        self.message = Some(Arc::from(
            "Archive change undone. Git branches kept; saving",
        ));
        self.reproject();
        Action::PersistConfig(self.register_config_mutation(mutation, common_dir))
    }
}
