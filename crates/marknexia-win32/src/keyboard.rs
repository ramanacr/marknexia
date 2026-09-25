//! Translate shell-owned key chords without intercepting Windows shortcuts.

const VK_TAB: u16 = 0x09;
const VK_W: u16 = 0x57;
const VK_F6: u16 = 0x75;
const VK_LEFT: u16 = 0x25;
const VK_RIGHT: u16 = 0x27;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyChord {
    virtual_key: u16,
    ctrl: bool,
    shift: bool,
    alt: bool,
}

impl KeyChord {
    #[must_use]
    pub fn new(virtual_key: u16, ctrl: bool, shift: bool, alt: bool) -> Self {
        Self {
            virtual_key,
            ctrl,
            shift,
            alt,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellCommand {
    NextTab,
    PreviousTab,
    CloseTab,
    CycleFocus,
}

#[must_use]
pub fn route_key(chord: KeyChord) -> Option<ShellCommand> {
    match chord {
        KeyChord {
            virtual_key: VK_TAB,
            ctrl: true,
            shift: false,
            alt: false,
        } => Some(ShellCommand::NextTab),
        KeyChord {
            virtual_key: VK_TAB,
            ctrl: true,
            shift: true,
            alt: false,
        } => Some(ShellCommand::PreviousTab),
        KeyChord {
            virtual_key: VK_W,
            ctrl: true,
            shift: false,
            alt: false,
        } => Some(ShellCommand::CloseTab),
        KeyChord {
            virtual_key: VK_F6,
            ctrl: false,
            shift: false,
            alt: false,
        } => Some(ShellCommand::CycleFocus),
        KeyChord {
            virtual_key: VK_LEFT,
            ctrl: false,
            shift: false,
            alt: false,
        } => Some(ShellCommand::PreviousTab),
        KeyChord {
            virtual_key: VK_RIGHT,
            ctrl: false,
            shift: false,
            alt: false,
        } => Some(ShellCommand::NextTab),
        _ => None,
    }
}
