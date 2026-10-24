//! Mouse input the app acts on. The app converts terminal cells into the
//! grid cells of the editor panel before it sends them on.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press(MouseButton),
    Drag(MouseButton),
    Release(MouseButton),
    ScrollUp,
    ScrollDown,
}

/// A mouse event over the editor panel, at a cell of Neovim's grid (row and
/// column counted from the panel's top left).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorMouse {
    pub action: MouseAction,
    pub row: u16,
    pub col: u16,
}
