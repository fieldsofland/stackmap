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
    Character(char),
    Quit,
    Ignored,
}

impl Key {
    fn from_event(event: KeyEvent) -> Self {
        if event.modifiers.contains(KeyModifiers::CONTROL) && event.code == KeyCode::Char('c') {
            return Self::Quit;
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
            KeyCode::Char('k') => Self::Up,
            KeyCode::Char('j') => Self::Down,
            KeyCode::Char('K') => Self::StackUp,
            KeyCode::Char('J') => Self::StackDown,
            KeyCode::Char('g') => Self::SectionUp,
            KeyCode::Char('G') => Self::SectionDown,
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
