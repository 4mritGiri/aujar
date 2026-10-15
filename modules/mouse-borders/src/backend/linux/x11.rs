//! Screen size and pointer position from an X11 server.
//!
//! This is what makes switching at screen edges possible: on X11 any client may ask where
//! the pointer is. (Wayland deliberately forbids it.)

use std::sync::atomic::{AtomicBool, Ordering};

use aujar_core::{Position, Size};
use x11rb::{
    connection::Connection,
    protocol::{xfixes::ConnectionExt as _, xproto::ConnectionExt as _},
    rust_connection::RustConnection,
};

use crate::{
    backend::Desktop,
    error::{Error, Result},
};

pub struct X11Desktop {
    connection: RustConnection,
    root: u32,
    size: Size,
    cursor_hidden: AtomicBool,
}

impl X11Desktop {
    pub fn connect() -> Result<Self> {
        let (connection, screen_number) = RustConnection::connect(None)
            .map_err(|error| Error::Other(format!("cannot connect to the X server: {error}")))?;
        let screen = &connection.setup().roots[screen_number];

        Ok(Self {
            root: screen.root,
            size: Size {
                width: u32::from(screen.width_in_pixels),
                height: u32::from(screen.height_in_pixels),
            },
            connection,
            cursor_hidden: AtomicBool::new(false),
        })
    }
}

impl Desktop for X11Desktop {
    fn screen_size(&self) -> Result<Size> {
        // Re-read each time: monitors come and go. A failed query falls back to the size
        // seen at connection time.
        let queried = self
            .connection
            .get_geometry(self.root)
            .ok()
            .and_then(|cookie| cookie.reply().ok());

        Ok(queried.map_or(self.size, |geometry| Size {
            width: u32::from(geometry.width),
            height: u32::from(geometry.height),
        }))
    }

    fn pointer(&self) -> Result<Position> {
        let reply = self
            .connection
            .query_pointer(self.root)
            .map_err(|error| Error::Other(error.to_string()))?
            .reply()
            .map_err(|error| Error::Other(error.to_string()))?;

        Ok(Position {
            x: i32::from(reply.root_x),
            y: i32::from(reply.root_y),
        })
    }

    fn set_cursor_visible(&self, visible: bool) -> Result<()> {
        // XFixes hide/show calls nest; keep them balanced.
        if self.cursor_hidden.load(Ordering::SeqCst) != visible {
            return Ok(());
        }

        let to_error = |error: &dyn std::fmt::Display| Error::Other(error.to_string());

        self.connection
            .xfixes_query_version(4, 0)
            .map_err(|e| to_error(&e))?
            .reply()
            .map_err(|e| to_error(&e))?;

        if visible {
            self.connection
                .xfixes_show_cursor(self.root)
                .map_err(|e| to_error(&e))?;
        } else {
            self.connection
                .xfixes_hide_cursor(self.root)
                .map_err(|e| to_error(&e))?;
        }

        self.connection.flush().map_err(|e| to_error(&e))?;
        self.cursor_hidden.store(!visible, Ordering::SeqCst);

        Ok(())
    }
}
