use nexora_core::{Position, Rect, Size, WindowId};

pub trait WindowManager {
    fn list_windows(&self) -> Vec<WindowId>;
    fn activate_window(&self, id: WindowId) -> anyhow::Result<()>;
    fn move_window(&self, id: WindowId, position: Position) -> anyhow::Result<()>;
    fn resize_window(&self, id: WindowId, size: Size) -> anyhow::Result<()>;
    fn set_always_on_top(&self, id: WindowId, enabled: bool) -> anyhow::Result<()>;
    fn geometry(&self, id: WindowId) -> anyhow::Result<Rect>;
}
