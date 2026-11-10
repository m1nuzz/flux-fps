//! System tray icon with quick actions, on a dedicated message-only window.
//!
//! windows-reactor does not expose HWNDs, so the tray lives on its own
//! thread with a classic Win32 message loop (hidden `HWND_MESSAGE` window).
//! UI -> Engine rules still apply: the tray sends `Command`s through the
//! same bounded mpsc with `try_send` only - never blocks, never touches
//! game processes. This is the user-mode resident binary: no elevation,
//! no driver code (CI import-table audit).

use common::Command;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::Sender;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, LoadIconW, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, TrackPopupMenu, TranslateMessage, HWND_MESSAGE, IDI_APPLICATION,
    MF_SEPARATOR, MF_STRING, MSG, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON, TRACK_POPUP_MENU_FLAGS, WM_APP,
    WM_COMMAND, WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW,
};

/// Custom messages for the tray window.
const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
const WM_TRAY_QUIT: u32 = WM_APP + 2;
/// Menu item ids (low word of WM_COMMAND wParam).
const MENU_PURGE: usize = 1;
const MENU_QUIT: usize = 2;

/// Handle to request tray teardown from the UI thread.
pub struct TrayHandle {
    hwnd: std::sync::Arc<AtomicIsize>,
}

impl TrayHandle {
    /// Posts a quit request to the tray message loop (non-blocking).
    pub fn shutdown(&self) {
        let hwnd = self.hwnd.load(Ordering::Acquire);
        if hwnd != 0 {
            // SAFETY: hwnd was created and lives on the tray thread; posting
            // is safe from any thread.
            unsafe {
                PostMessageWTray(hwnd as HWND, WM_TRAY_QUIT);
            }
        }
    }
}

/// Converts a Rust string to a null-terminated UTF-16 buffer.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Spawns the tray thread. Best-effort: on failure the process still runs,
/// the tray icon is simply absent.
pub fn spawn(commands: Sender<Command>) -> TrayHandle {
    let hwnd_slot = std::sync::Arc::new(AtomicIsize::new(0));
    let slot = hwnd_slot.clone();
    let spawned = std::thread::Builder::new()
        .name("flux-fps-tray".into())
        .spawn(move || tray_thread(commands, slot));
    if let Err(e) = spawned {
        eprintln!("[flux-fps] tray thread failed to start: {e}");
    }
    TrayHandle { hwnd: hwnd_slot }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: standard window procedure contract.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// PostMessageW wrapper avoiding unused-parameter lint noise in shutdown().
unsafe fn PostMessageWTray(hwnd: HWND, msg: u32) {
    // SAFETY: valid hwnd owned by the tray thread.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd, msg, 0, 0);
    }
}

fn tray_thread(commands: Sender<Command>, hwnd_slot: std::sync::Arc<AtomicIsize>) {
    const CLASS_NAME: &[u16] = &utf16_static("flux-fps-tray");

    const fn utf16_static(s: &str) -> [u16; 15] {
        // Class name is fixed and short; trailing zeros pad the rest.
        let mut out = [0u16; 15];
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() && i < 14 {
            out[i] = bytes[i] as u16;
            i += 1;
        }
        out
    }

    // SAFETY: classic window class registration with static class name.
    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: std::ptr::null_mut(),
        hIcon: unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) },
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: CLASS_NAME.as_ptr(),
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        eprintln!("[flux-fps] tray: RegisterClassW failed");
        return;
    }

    // SAFETY: message-only window (HWND_MESSAGE parent), no styles needed.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            CLASS_NAME.as_ptr(),
            CLASS_NAME.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        eprintln!("[flux-fps] tray: CreateWindowExW failed");
        return;
    }
    hwnd_slot.store(hwnd as isize, Ordering::Release);

    let tip = wide("flux-fps");
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY_CALLBACK;
    nid.hIcon = unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) };
    for (dst, src) in nid.szTip.iter_mut().zip(tip) {
        *dst = src;
    }
    // SAFETY: nid is fully initialized above.
    if unsafe { Shell_NotifyIconW(NIM_ADD, &nid) } == 0 {
        eprintln!("[flux-fps] tray: Shell_NotifyIconW(NIM_ADD) failed");
        return;
    }

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: standard message loop over a valid window.
        let ret = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if ret <= 0 {
            break; // WM_QUIT or error
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Dispatch tray-specific logic after the standard pump.
        match msg.message {
            WM_TRAY_QUIT => {
                // SAFETY: our window; removing the icon before closing.
                unsafe {
                    Shell_NotifyIconW(NIM_DELETE, &nid);
                    DestroyWindow(hwnd);
                }
                hwnd_slot.store(0, Ordering::Release);
                unsafe { PostQuitMessage(0) };
            }
            WM_TRAY_CALLBACK => {
                let mouse = (msg.lParam & 0xFFFF) as u32;
                match mouse {
                    WM_LBUTTONUP => {
                        send(&commands, Command::PurgeStandbyNow);
                    }
                    WM_RBUTTONUP => {
                        show_menu(hwnd, &commands);
                    }
                    _ => {}
                }
            }
            WM_COMMAND => {
                let id = (msg.wParam & 0xFFFF) as usize;
                match id {
                    MENU_PURGE => send(&commands, Command::PurgeStandbyNow),
                    MENU_QUIT => {
                        // Engine receives a graceful shutdown via the UI.
                        send(&commands, Command::Shutdown);
                        // SAFETY: our window.
                        unsafe {
                            Shell_NotifyIconW(NIM_DELETE, &nid);
                            DestroyWindow(hwnd);
                        }
                        hwnd_slot.store(0, Ordering::Release);
                        unsafe { PostQuitMessage(0) };
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if msg.message == WM_TRAY_QUIT
            || (msg.message == WM_COMMAND && (msg.wParam & 0xFFFF) as usize == MENU_QUIT)
        {
            break;
        }
    }
}

fn show_menu(hwnd: HWND, commands: &Sender<Command>) {
    // SAFETY: menu lifecycle within this function; hwnd is valid.
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        let purge = wide("Purge standby now");
        let quit = wide("Quit flux-fps");
        AppendMenuW(menu, MF_STRING, MENU_PURGE, purge.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(menu, MF_STRING, MENU_QUIT, quit.as_ptr());
        // Required so the menu closes when the user clicks elsewhere.
        SetForegroundWindow(hwnd);
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        TrackPopupMenu(
            menu,
            (TPM_RIGHTBUTTON | TPM_BOTTOMALIGN) as TRACK_POPUP_MENU_FLAGS,
            pt.x,
            pt.y,
            0,
            hwnd,
            std::ptr::null(),
        );
        DestroyMenu(menu);
        let _ = commands;
    }
}

fn send(commands: &Sender<Command>, command: Command) {
    if commands.try_send(command).is_err() {
        eprintln!("[flux-fps] tray: command queue full, dropped");
    }
}
