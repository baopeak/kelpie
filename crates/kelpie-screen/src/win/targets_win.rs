//! Top-level windows as `targets` wants them (front to back), where a window is now, and the cursor.

use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetCursorInfo, GetWindow, GetWindowLongPtrW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, LoadCursorW, CURSORINFO, CURSOR_SHOWING,
    GWL_EXSTYLE, GW_OWNER, HCURSOR, IDC_APPSTARTING, IDC_ARROW, IDC_CROSS, IDC_HAND, IDC_IBEAM, IDC_NO, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE, IDC_WAIT, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

use super::from_wide;
use crate::geom::Rect;
use crate::targets::WindowInfo;
use crate::wire::Shape;

pub fn hwnd(h: u64) -> HWND {
    HWND(h as usize as *mut _)
}

/// Where a window is: its frame as DWM draws it (no invisible resize borders), physical pixels.
pub fn window_rect(h: HWND) -> Option<Rect> {
    let mut r = RECT::default();
    unsafe { DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, std::mem::size_of::<RECT>() as u32).ok()? };
    Some(Rect::from_edges(r.left, r.top, r.right, r.bottom))
}

pub fn process_of(h: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    pid
}

pub fn exists(h: HWND, pid: u32) -> bool {
    unsafe { IsWindow(Some(h)).as_bool() } && process_of(h) == pid
}

pub fn minimized(h: HWND) -> bool {
    unsafe { IsIconic(h).as_bool() }
}

fn exe_name(pid: u32) -> String {
    unsafe {
        let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(p);
        if !ok {
            return String::new();
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        full.rsplit(['\\', '/']).next().unwrap_or_default().to_string()
    }
}

pub fn info(h: HWND) -> WindowInfo {
    unsafe {
        let mut title = [0u16; 512];
        let n = GetWindowTextW(h, &mut title);
        let mut class = [0u16; 256];
        let c = GetClassNameW(h, &mut class);
        let pid = process_of(h);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4);
        WindowInfo {
            hwnd: h.0 as usize as u64,
            pid,
            title: from_wide(&title[..n.max(0) as usize]),
            class: from_wide(&class[..c.max(0) as usize]),
            exe: exe_name(pid),
            rect: window_rect(h).unwrap_or_default(),
            visible: IsWindowVisible(h).as_bool(),
            cloaked: cloaked != 0,
            tool: ex & WS_EX_TOOLWINDOW.0 != 0 && ex & WS_EX_APPWINDOW.0 == 0,
            owned: GetWindow(h, GW_OWNER).is_ok_and(|o| !o.is_invalid()),
            minimized: IsIconic(h).as_bool(),
        }
    }
}

unsafe extern "system" fn collect(h: HWND, data: LPARAM) -> BOOL {
    let list = &mut *(data.0 as *mut Vec<HWND>);
    list.push(h);
    BOOL::from(list.len() < 1024)
}

/// Every top-level window, front to back.
pub fn windows() -> Vec<WindowInfo> {
    let mut handles: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut handles as *mut _ as isize));
    }
    handles.into_iter().map(info).collect()
}

/// The cursor: shape, whether it shows, where (screen pixels).
pub fn cursor() -> (Shape, bool, i32, i32) {
    unsafe {
        let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
        if GetCursorInfo(&mut ci).is_err() {
            return (Shape::Arrow, false, 0, 0);
        }
        let POINT { x, y } = ci.ptScreenPos;
        (shape_of(ci.hCursor), ci.flags.0 & CURSOR_SHOWING.0 != 0, x, y)
    }
}

fn shape_of(c: HCURSOR) -> Shape {
    let table = [
        (IDC_IBEAM, Shape::IBeam),
        (IDC_HAND, Shape::Hand),
        (IDC_WAIT, Shape::Wait),
        (IDC_CROSS, Shape::Cross),
        (IDC_SIZENS, Shape::SizeNs),
        (IDC_SIZEWE, Shape::SizeWe),
        (IDC_SIZENWSE, Shape::SizeNwse),
        (IDC_SIZENESW, Shape::SizeNesw),
        (IDC_SIZEALL, Shape::SizeAll),
        (IDC_NO, Shape::No),
        (IDC_APPSTARTING, Shape::AppStarting),
        (IDC_ARROW, Shape::Arrow),
    ];
    for (id, shape) in table {
        // The system's shared cursors: the same handle every time.
        if unsafe { LoadCursorW(None, id) }.is_ok_and(|h| h == c) {
            return shape;
        }
    }
    Shape::Arrow
}
