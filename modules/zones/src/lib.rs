use aujar_core::{Position, Rect, Size};

#[derive(Debug, Clone)]
pub struct ZoneGrid {
    pub columns: u32,
    pub rows: u32,
}

impl ZoneGrid {
    pub fn new(columns: u32, rows: u32) -> Self {
        assert!(columns > 0 && rows > 0);
        Self { columns, rows }
    }

    pub fn cell(&self, screen: Rect, column: u32, row: u32) -> Option<Rect> {
        if column >= self.columns || row >= self.rows {
            return None;
        }

        let cell_width = screen.size.width / self.columns;
        let cell_height = screen.size.height / self.rows;

        Some(Rect {
            position: Position {
                x: screen.position.x + (column * cell_width) as i32,
                y: screen.position.y + (row * cell_height) as i32,
            },
            size: Size {
                width: cell_width,
                height: cell_height,
            },
        })
    }
}
