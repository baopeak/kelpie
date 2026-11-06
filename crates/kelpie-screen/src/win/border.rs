//! What the desktop shows while a phone sees it: an always-on-top orange border around what is shared, with a badge naming the phone and
//! the stop hotkey. The window lets every click through, never takes focus, and is left out of every capture (`WDA_EXCLUDEFROMCAPTURE`,
//! Windows 10 2004+), so the phone never sees it. The stop hotkey is registered on the same window's thread (`RegisterHotKey`), so it works
//! whatever has the focus and even when the app's window is hung.

use std::cell::RefCell;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, InvalidateRect, SetBkMode, SetTextColor, DT_CENTER, DT_SINGLELINE, DT_VCENTER, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW, PostMessageW, PostQuitMessage, RegisterClassW, SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowPos,
    ShowWindow, TranslateMessage, HWND_TOPMOST, LWA_COLORKEY, MSG, SWP_NOACTIVATE, SW_SHOWNOACTIVATE, WDA_EXCLUDEFROMCAPTURE, WM_APP, WM_CLOSE, WM_DESTROY, WM_HOTKEY, WM_PAINT, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use super::wide;
use crate::geom::Rect;
use crate::input::Hotkey;

const KEY: COLORREF = COLORREF(0x00FF_00FF);
const ORANGE: COLORREF = COLORREF(0x0000_8CFF);
const DARK: COLORREF = COLORREF(0x0023_1814);
const THICK: i32 = 4;
const BADGE_W: i32 = 420;
const BADGE_H: i32 = 26;
const MOVE: u32 = WM_APP + 1;

struct State {
    label: Vec<u16>,
    rect: Arc<Mutex<Rect>>,
    hotkey: Option<Sender<()>>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

pub struct Border {
    hwnd: Arc<Mutex<usize>>,
    rect: Arc<Mutex<Rect>>,
    thread: Option<JoinHandle<()>>,
}

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let key = CreateSolidBrush(KEY);
            let orange = CreateSolidBrush(ORANGE);
            let dark = CreateSolidBrush(DARK);
            FillRect(dc, &client, key);
            for edge in [
                RECT { left: 0, top: 0, right: client.right, bottom: THICK },
                RECT { left: 0, top: client.bottom - THICK, right: client.right, bottom: client.bottom },
                RECT { left: 0, top: 0, right: THICK, bottom: client.bottom },
                RECT { left: client.right - THICK, top: 0, right: client.right, bottom: client.bottom },
            ] {
                FillRect(dc, &edge, orange);
            }
            let w = BADGE_W.min(client.right - 2 * THICK);
            let mut badge = RECT { left: (client.right - w) / 2, top: THICK, right: (client.right + w) / 2, bottom: THICK + BADGE_H };
            FillRect(dc, &badge, dark);
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, ORANGE);
            STATE.with(|s| {
                if let Some(s) = s.borrow().as_ref() {
                    let mut text = s.label.clone();
                    text.pop();
                    DrawTextW(dc, &mut text, &mut badge, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                }
            });
            let _ = DeleteObject(key.into());
            let _ = DeleteObject(orange.into());
            let _ = DeleteObject(dark.into());
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        MOVE => {
            let r = STATE.with(|s| s.borrow().as_ref().map(|s| *s.rect.lock().unwrap()));
            if let Some(r) = r {
                let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), r.x - THICK, r.y - THICK, r.w + 2 * THICK, r.h + 2 * THICK, SWP_NOACTIVATE);
                let _ = InvalidateRect(Some(hwnd), None, true);
            }
            LRESULT(0)
        }
        WM_HOTKEY => {
            STATE.with(|s| {
                if let Some(tx) = s.borrow().as_ref().and_then(|s| s.hotkey.as_ref()) {
                    let _ = tx.send(());
                }
            });
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = UnregisterHotKey(Some(hwnd), 1);
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

impl Border {
    /// Shows the border around `rect` (screen pixels) with `label` in its badge, and registers the stop hotkey, which sends on `on_hotkey`. Returns
    /// once the window exists: an error (no window, or the hotkey held by another program) means the screen must not be shared, since the desktop
    /// would not show it or could not stop it.
    pub fn show(rect: Rect, label: &str, hotkey: Hotkey, on_hotkey: Sender<()>) -> Result<Border, String> {
        let hwnd = Arc::new(Mutex::new(0usize));
        let shared = Arc::new(Mutex::new(rect));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let (h, r) = (hwnd.clone(), shared.clone());
        let label = wide(label);
        let thread = std::thread::spawn(move || unsafe {
            STATE.with(|s| *s.borrow_mut() = Some(State { label, rect: r.clone(), hotkey: Some(on_hotkey) }));
            let instance = GetModuleHandleW(None).unwrap_or_default();
            let class = wide(crate::BORDER_CLASS);
            let wc = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: instance.into(), lpszClassName: PCWSTR(class.as_ptr()), ..Default::default() };
            RegisterClassW(&wc);
            let rect = *r.lock().unwrap();
            let Ok(win) = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                PCWSTR(class.as_ptr()),
                w!("Kelpie screen sharing"),
                WS_POPUP,
                rect.x - THICK,
                rect.y - THICK,
                rect.w + 2 * THICK,
                rect.h + 2 * THICK,
                None,
                None,
                Some(instance.into()),
                None,
            ) else {
                let _ = ready_tx.send(Err("the sharing border could not be shown".into()));
                return;
            };
            *h.lock().unwrap() = win.0 as usize;
            let _ = SetLayeredWindowAttributes(win, KEY, 255, LWA_COLORKEY);
            let _ = SetWindowDisplayAffinity(win, WDA_EXCLUDEFROMCAPTURE);
            let _ = ShowWindow(win, SW_SHOWNOACTIVATE);
            if RegisterHotKey(Some(win), 1, HOT_KEY_MODIFIERS(hotkey.mods) | MOD_NOREPEAT, u32::from(hotkey.vk)).is_err() {
                let _ = DestroyWindow(win);
                let _ = ready_tx.send(Err("another program holds the stop hotkey: pick another in Settings → Mobile".into()));
                return;
            }
            let _ = ready_tx.send(Ok(()));
            let mut msg = MSG::default();
            // `GetMessageW` is -1 on an error, which is not a message: the loop ends then too.
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        });
        match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Border { hwnd, rect: shared, thread: Some(thread) }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("the sharing border did not appear".into()),
        }
    }
    fn post(&self, msg: u32) {
        let h = *self.hwnd.lock().unwrap();
        if h != 0 {
            unsafe {
                let _ = PostMessageW(Some(HWND(h as *mut _)), msg, WPARAM(0), LPARAM(0));
            }
        }
    }

    /// The target moved or was resized.
    pub fn place(&self, rect: Rect) {
        let changed = {
            let mut r = self.rect.lock().unwrap();
            std::mem::replace(&mut *r, rect) != rect
        };
        if changed {
            self.post(MOVE);
        }
    }
}

impl Drop for Border {
    fn drop(&mut self) {
        self.post(WM_CLOSE);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
