//! Windows: the virtual screen with GDI, monitors with their DPI, and windows from DWM.
//!
//! The capturing thread is made per-monitor DPI aware (v2), so every coordinate Windows gives is
//! in physical pixels; logical sizes come from each monitor's effective DPI (96 is 100 %).

use crate::{Monitor, Screen, windows::WindowRect};
use anyhow::{Result, bail};
use std::{ffi::c_void, mem::size_of};
use windows_sys::Win32::{
    Foundation::{BOOL, CloseHandle, HWND, LPARAM, RECT, TRUE},
    Graphics::{
        Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
        Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
            CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC,
            GetDIBits, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, ReleaseDC, SRCCOPY,
            SelectObject,
        },
    },
    System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    },
    UI::{
        HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
            SetThreadDpiAwarenessContext,
        },
        WindowsAndMessaging::{
            EnumWindows, GWL_EXSTYLE, GetSystemMetrics, GetWindowLongW, GetWindowTextLengthW,
            GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
            SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
            WS_EX_TOOLWINDOW,
        },
    },
};

/// Physical pixels on the virtual screen.
#[derive(Debug, Clone, Copy)]
struct Physical {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    scale: f32,
}

/// Per-monitor DPI awareness for this thread while it lives, then the previous mode again.
struct DpiAware(isize);

impl DpiAware {
    fn enter() -> Self {
        // SAFETY: changes only this thread's DPI mode; the old one is restored on drop.
        Self(
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
                as isize,
        )
    }
}

impl Drop for DpiAware {
    fn drop(&mut self) {
        if self.0 != 0 {
            // SAFETY: restores the context returned by `enter`.
            unsafe { SetThreadDpiAwarenessContext(self.0 as _) };
        }
    }
}

unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _dc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // SAFETY: `data` is the `Vec<Physical>` passed by `physical_monitors`.
    let out = unsafe { &mut *(data as *mut Vec<Physical>) };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;

    // SAFETY: `info` is a valid, sized MONITORINFO.
    if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        // SAFETY: plain out-parameters.
        let scale =
            if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) } >= 0
            {
                dpi_x as f32 / 96.0
            } else {
                1.0
            };

        out.push(Physical {
            left: info.rcMonitor.left,
            top: info.rcMonitor.top,
            right: info.rcMonitor.right,
            bottom: info.rcMonitor.bottom,
            scale: scale.max(0.5),
        });
    }

    TRUE
}

fn physical_monitors() -> Vec<Physical> {
    let mut out: Vec<Physical> = Vec::new();

    // SAFETY: the callback only writes into `out`, which outlives the call.
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect_monitor),
            &mut out as *mut Vec<Physical> as LPARAM,
        );
    }

    out
}

/// Monitors in logical pixels the way Windows lays them out for apps: each monitor's origin
/// keeps its physical position, its size is divided by its own scale.
fn to_monitor(m: &Physical, origin: (i32, i32)) -> Monitor {
    Monitor {
        x: m.left as f32 / m.scale,
        y: m.top as f32 / m.scale,
        width: (m.right - m.left) as f32 / m.scale,
        height: (m.bottom - m.top) as f32 / m.scale,
        picture_x: (m.left - origin.0).max(0) as u32,
        picture_y: (m.top - origin.1).max(0) as u32,
        picture_width: (m.right - m.left).max(0) as u32,
        picture_height: (m.bottom - m.top).max(0) as u32,
    }
}

pub(crate) fn capture() -> Result<Screen> {
    let _dpi = DpiAware::enter();

    // SAFETY: plain metric queries.
    let (x, y, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };

    if width <= 0 || height <= 0 {
        bail!("Windows reported an empty screen");
    }

    let mut bgra = vec![0u8; width as usize * height as usize * 4];

    // SAFETY: standard GDI screen copy; every handle created here is released below, and
    // `bgra` is exactly width * height * 4 bytes for the 32-bit top-down DIB requested.
    let copied = unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let memory = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let previous = SelectObject(memory, bitmap as _);

        let blitted = BitBlt(
            memory,
            0,
            0,
            width,
            height,
            screen,
            x,
            y,
            SRCCOPY | CAPTUREBLT,
        );

        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Negative: rows top to bottom.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };

        let rows = GetDIBits(
            memory,
            bitmap,
            0,
            height as u32,
            bgra.as_mut_ptr() as *mut c_void,
            &mut info,
            DIB_RGB_COLORS,
        );

        SelectObject(memory, previous);
        DeleteObject(bitmap as _);
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut(), screen);

        blitted != 0 && rows == height
    };

    if !copied {
        bail!("Windows could not copy the screen");
    }

    // BGRA (alpha undefined for the screen) to opaque RGBA.
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }

    let picture = image::RgbaImage::from_raw(width as u32, height as u32, bgra)
        .ok_or_else(|| anyhow::anyhow!("the screen picture has the wrong size"))?;
    let monitors = physical_monitors()
        .iter()
        .map(|monitor| to_monitor(monitor, (x, y)))
        .collect();

    Ok(Screen { picture, monitors })
}

fn window_text(hwnd: HWND) -> String {
    // SAFETY: the buffer is sized from GetWindowTextLengthW plus the terminator.
    unsafe {
        let length = GetWindowTextLengthW(hwnd);

        if length <= 0 {
            return String::new();
        }

        let mut buffer = vec![0u16; length as usize + 1];
        let copied = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);

        String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
    }
}

/// The program's file name without `.exe` (`Code`, `firefox`), as the window's app.
fn window_app(hwnd: HWND) -> String {
    // SAFETY: the process handle is closed; the buffer size is passed in and updated.
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);

        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);

        if process.is_null() {
            return String::new();
        }

        let mut buffer = vec![0u16; 1024];
        let mut size = buffer.len() as u32;
        let ok =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut size);

        CloseHandle(process);

        if ok == 0 {
            return String::new();
        }

        let path = String::from_utf16_lossy(&buffer[..size as usize]);
        let name = path.rsplit(['\\', '/']).next().unwrap_or_default();

        name.strip_suffix(".exe")
            .or_else(|| name.strip_suffix(".EXE"))
            .unwrap_or(name)
            .to_owned()
    }
}

struct Collect {
    monitors: Vec<Physical>,
    windows: Vec<WindowRect>,
}

unsafe extern "system" fn collect_window(hwnd: HWND, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the `Collect` passed by `visible_windows`.
    let collect = unsafe { &mut *(data as *mut Collect) };

    // SAFETY: read-only window queries.
    unsafe {
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return TRUE;
        }

        if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 {
            return TRUE;
        }

        // Windows on other virtual desktops and suspended UWP frames are "cloaked".
        let mut cloaked = 0u32;
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            &mut cloaked as *mut u32 as *mut c_void,
            size_of::<u32>() as u32,
        );

        if cloaked != 0 {
            return TRUE;
        }

        // The visible frame, without the invisible resize borders.
        let mut frame: RECT = std::mem::zeroed();

        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut frame as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        ) < 0
        {
            return TRUE;
        }

        let title = window_text(hwnd);

        if title.is_empty() || frame.right <= frame.left || frame.bottom <= frame.top {
            return TRUE;
        }

        // Logical pixels with the scale of the monitor the window's center is on.
        let center = (
            (frame.left + frame.right) / 2,
            (frame.top + frame.bottom) / 2,
        );
        let scale = collect
            .monitors
            .iter()
            .find(|m| {
                center.0 >= m.left && center.0 < m.right && center.1 >= m.top && center.1 < m.bottom
            })
            .map_or(1.0, |m| m.scale);

        // EnumWindows goes from the top of the z-order down.
        let stack = -(collect.windows.len() as i32);

        collect.windows.push(WindowRect {
            title,
            app: window_app(hwnd),
            x: (frame.left as f32 / scale).round() as i32,
            y: (frame.top as f32 / scale).round() as i32,
            width: ((frame.right - frame.left) as f32 / scale).round() as i32,
            height: ((frame.bottom - frame.top) as f32 / scale).round() as i32,
            stack,
        });
    }

    TRUE
}

pub(crate) fn visible_windows() -> Vec<WindowRect> {
    let _dpi = DpiAware::enter();
    let mut collect = Collect {
        monitors: physical_monitors(),
        windows: Vec::new(),
    };

    // SAFETY: the callback only writes into `collect`, which outlives the call.
    unsafe {
        EnumWindows(Some(collect_window), &mut collect as *mut Collect as LPARAM);
    }

    collect.windows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_dpi_monitors_keep_their_pixels_and_logical_size() {
        // A 4K laptop at 200 % left of a 1080p monitor at 100 %.
        let laptop = Physical {
            left: 0,
            top: 0,
            right: 3840,
            bottom: 2160,
            scale: 2.0,
        };
        let external = Physical {
            left: 3840,
            top: 0,
            right: 5760,
            bottom: 1080,
            scale: 1.0,
        };

        let a = to_monitor(&laptop, (0, 0));
        let b = to_monitor(&external, (0, 0));

        assert_eq!((a.width, a.height), (1920.0, 1080.0));
        assert_eq!((a.picture_width, a.picture_height), (3840, 2160));
        assert_eq!((b.picture_x, b.picture_width), (3840, 1920));
        assert_eq!(b.width, 1920.0);
    }
}
