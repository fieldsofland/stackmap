use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyPhase {
    Press,
    Repeat,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Input {
    pub key: Key,
    pub phase: KeyPhase,
}

impl Input {
    pub const fn new(key: Key, phase: KeyPhase) -> Self {
        Self { key, phase }
    }

    pub const fn press(key: Key) -> Self {
        Self::new(key, KeyPhase::Press)
    }

    pub const fn repeat(key: Key) -> Self {
        Self::new(key, KeyPhase::Repeat)
    }

    pub fn from_event(event: KeyEvent) -> Option<Self> {
        let phase = match event.kind {
            KeyEventKind::Press => KeyPhase::Press,
            KeyEventKind::Repeat => KeyPhase::Repeat,
            KeyEventKind::Release => return None,
        };
        Some(Self::new(Key::from_event(event), phase))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Key {
    Up,
    Down,
    StackUp,
    StackDown,
    SectionUp,
    SectionDown,
    Enter,
    Escape,
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    Tab,
    ClearNameDraft,
    CopyBranch,
    CopySection,
    CopyStack,
    Character(char),
    Quit,
    Ignored,
}

impl Key {
    fn from_event(event: KeyEvent) -> Self {
        if event.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(event.code, KeyCode::Char('u') | KeyCode::Char('U'))
        {
            return Self::ClearNameDraft;
        }
        if event.modifiers.contains(KeyModifiers::CONTROL) && event.code == KeyCode::Char('c') {
            return Self::Quit;
        }
        if event.modifiers == KeyModifiers::SHIFT && event.code == KeyCode::Backspace {
            return Self::ClearNameDraft;
        }
        if matches!(event.code, KeyCode::Char('c') | KeyCode::Char('C')) {
            if event.modifiers == KeyModifiers::SUPER | KeyModifiers::ALT | KeyModifiers::SHIFT {
                return Self::CopyStack;
            }
            if event.modifiers == KeyModifiers::SUPER | KeyModifiers::SHIFT {
                return Self::CopySection;
            }
            if event.modifiers == KeyModifiers::SUPER {
                return Self::CopyBranch;
            }
        }
        match event.code {
            KeyCode::Up if event.modifiers.contains(KeyModifiers::ALT) => Self::SectionUp,
            KeyCode::Down if event.modifiers.contains(KeyModifiers::ALT) => Self::SectionDown,
            KeyCode::Up
                if event
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::SUPER) =>
            {
                Self::StackUp
            }
            KeyCode::Down
                if event
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::SUPER) =>
            {
                Self::StackDown
            }
            KeyCode::Up => Self::Up,
            KeyCode::Down => Self::Down,
            KeyCode::Enter => Self::Enter,
            KeyCode::Esc => Self::Escape,
            KeyCode::Backspace => Self::Backspace,
            KeyCode::Delete => Self::Delete,
            KeyCode::Left => Self::Left,
            KeyCode::Right => Self::Right,
            KeyCode::Home => Self::Home,
            KeyCode::End => Self::End,
            KeyCode::Tab => Self::Tab,
            KeyCode::Char(character) => Self::Character(character),
            _ => Self::Ignored,
        }
    }

    pub const fn allows_repeat(&self) -> bool {
        matches!(
            self,
            Self::Up
                | Self::Down
                | Self::StackUp
                | Self::StackDown
                | Self::SectionUp
                | Self::SectionDown
        )
    }
}
