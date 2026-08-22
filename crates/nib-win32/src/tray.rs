//! `Win32Tray` — the system-tray icon: left-click cycles the cleanup mode; right-click shows a
//! menu (current mode checked) + the overlay theme picker (applies live) + Quit. Runs on its own
//! thread with a hidden window + message loop. Emits `TrayCommand`s; reads/writes the shared
//! mode/style atoms so the menu, overlay, and pipeline stay in sync.

use std::sync::atomic::{AtomicIsize, AtomicU8, Ordering::SeqCst};
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    GetCursorPos, GetMessageW, GetSystemMetrics, LoadIconW, LoadImageW, PostMessageW,
    RegisterClassW, SetForegroundWindow, TrackPopupMenu, TranslateMessage, HICON, HMENU,
    IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR, MENU_ITEM_FLAGS, MF_CHECKED, MF_SEPARATOR,
    MF_STRING, MSG, SM_CXSMICON, SM_CYSMICON, TPM_RETURNCMD, TPM_RIGHTBUTTON, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
};

/// A user action from the tray. `nib-core` forwards these to the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    SetMode(u8),
    SetStyle(u8),
    CycleMode,
    Quit,
}

const CB_MSG: u32 = WM_APP + 1;
const ID_MODE_BASE: usize = 1001; // + mode index (0..=3)
const ID_QUIT: usize = 1009;
const ID_STYLE_BASE: usize = 1101; // + OverlayStyle::index()

const MODE_LABELS: [&str; 4] = [
    "Raw (verbatim)",
    "Auto-punctuation",
    "Polish",
    "Email / Formal",
];

static TRAY_TX: OnceLock<Sender<TrayCommand>> = OnceLock::new();
static TRAY_MODE: OnceLock<Arc<AtomicU8>> = OnceLock::new();
static TRAY_STYLE: OnceLock<Arc<AtomicU8>> = OnceLock::new();
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);

/// The tray icon (a ZST; the OS window/state lives in module statics on the tray thread).
#[derive(Debug, Default, Clone, Copy)]
pub struct Win32Tray;

impl Win32Tray {
    /// Spawn the tray on its own thread. `mode`/`style` are shared with the pipeline and overlay:
    /// the menu reads them to show the active item, and picking one updates them immediately.
    pub fn spawn(tx: Sender<TrayCommand>, mode: Arc<AtomicU8>, style: Arc<AtomicU8>) {
        let _ = TRAY_TX.set(tx);
        let _ = TRAY_MODE.set(mode);
        let _ = TRAY_STYLE.set(style);
        std::thread::spawn(|| unsafe { run() });
    }

    /// Install a console Ctrl+C / Ctrl+Break / close handler that runs `cleanup` (bounded), then
    /// removes the tray icon, then exits — so an abrupt exit doesn't leave a ghost icon OR an
    /// orphaned sidecar. `cleanup` should trigger the app's graceful-quit path and return once
    /// resources are released (or as fast as it can); the handler exits regardless when it
    /// returns. (The graceful q / tray-Quit paths don't go through here.)
    pub fn install_ctrl_handler(cleanup: impl Fn() + Send + Sync + 'static) {
        let _ = CTRL_CLEANUP.set(Box::new(cleanup));
        unsafe {
            let _ = SetConsoleCtrlHandler(Some(ctrl_handler), BOOL(1));
        }
    }

    /// Remove the tray icon. Callable from any thread — used by the quit path, which
    /// `process::exit`s without ending the tray thread's loop.
    pub fn delete_icon() {
        let h = TRAY_HWND.load(SeqCst);
        if h == 0 {
            return;
        }
        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: HWND(h as *mut core::ffi::c_void),
            uID: 1,
            ..Default::default()
        };
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        }
    }
}

/// The app icon embedded in our own executable (resource id 1 — see `nib-core`'s `build.rs`),
/// loaded at the size Windows wants for the notification area.
///
/// Asking for the small-icon metric rather than letting the shell downscale a 32 px copy is what
/// keeps it crisp: the `.ico` carries a purpose-drawn 16 px variant with fewer, fatter bars, and
/// `LoadImageW` picks that entry. Falls back to the stock application icon if the binary was built
/// without the resource, so a contributor without the Windows SDK still gets a working tray.
unsafe fn tray_icon(hinst: HINSTANCE) -> HICON {
    let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
    // MAKEINTRESOURCE(1): Win32 overloads this parameter, passing a small integer where a string
    // pointer would go. `without_provenance` says exactly that — an address that is deliberately
    // not a real pointer — which is both what the API means and what keeps clippy's
    // dangling-pointer lint honest instead of silenced.
    let id = PCWSTR(std::ptr::without_provenance(1));
    LoadImageW(hinst, id, IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR)
        .map(|h| HICON(h.0))
        .unwrap_or_else(|_| LoadIconW(None, IDI_APPLICATION).unwrap_or_default())
}

unsafe fn run() {
    let hinst = GetModuleHandleW(None).unwrap();
    let class: Vec<u16> = "NibTray\0".encode_utf16().collect();
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: HINSTANCE(hinst.0),
        lpszClassName: PCWSTR(class.as_ptr()),
        ..Default::default()
    };
    RegisterClassW(&wc);
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(class.as_ptr()),
        PCWSTR::null(),
        WINDOW_STYLE(0),
        0,
        0,
        0,
        0,
        HWND::default(),
        HMENU::default(),
        HINSTANCE(hinst.0),
        None,
    )
    .expect("tray window");
    TRAY_HWND.store(hwnd.0 as isize, SeqCst);

    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: CB_MSG,
        hIcon: tray_icon(HINSTANCE(hinst.0)),
        ..Default::default()
    };
    let tip: Vec<u16> = "HoldToSpeak — voice dictation (right-click for modes)\0"
        .encode_utf16()
        .collect();
    for (i, &c) in tip.iter().take(nid.szTip.len()).enumerate() {
        nid.szTip[i] = c;
    }
    let _ = Shell_NotifyIconW(NIM_ADD, &nid);

    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
}

static CTRL_CLEANUP: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

unsafe extern "system" fn ctrl_handler(_ctrl_type: u32) -> BOOL {
    // Run the app's cleanup first (e.g. quit the pipeline so Sidecar::Drop kills the child) —
    // exiting straight away would skip Drop and resurrect the orphaned-sidecar problem.
    if let Some(cleanup) = CTRL_CLEANUP.get() {
        cleanup();
    }
    Win32Tray::delete_icon();
    std::process::exit(0);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == CB_MSG {
        let mouse = (lp.0 as u32) & 0xFFFF;
        if mouse == WM_LBUTTONUP {
            send(TrayCommand::CycleMode);
        } else if mouse == WM_RBUTTONUP {
            show_menu(hwnd);
        }
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn send(cmd: TrayCommand) {
    if let Some(tx) = TRAY_TX.get() {
        let _ = tx.send(cmd);
    }
}

unsafe fn append(menu: HMENU, flags: MENU_ITEM_FLAGS, id: usize, label: &str) {
    let w: Vec<u16> = format!("{label}\0").encode_utf16().collect();
    let _ = AppendMenuW(menu, flags, id, PCWSTR(w.as_ptr()));
}

unsafe fn show_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let cur_mode = TRAY_MODE.get().map(|m| m.load(SeqCst)).unwrap_or(0);
    let chk = |i: u8| {
        if cur_mode == i {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        }
    };
    for (i, label) in MODE_LABELS.iter().enumerate() {
        append(menu, chk(i as u8), ID_MODE_BASE + i, label);
    }
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

    let cur_style = TRAY_STYLE.get().map(|s| s.load(SeqCst)).unwrap_or(0);
    let chk_s = |i: u8| {
        if cur_style == i {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        }
    };
    // Derived from OverlayStyle::ALL (display order) — adding a style picks it up automatically;
    // the menu id is always ID_STYLE_BASE + the style's index.
    for style in crate::OverlayStyle::ALL {
        let i = style.index();
        append(
            menu,
            chk_s(i),
            ID_STYLE_BASE + i as usize,
            style.menu_label(),
        );
    }
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    append(menu, MF_STRING, ID_QUIT, "Quit HoldToSpeak");

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd); // so the menu dismisses on click-away
    let cmd = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_RETURNCMD,
        pt.x,
        pt.y,
        0,
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
    // Documented TrackPopupMenu quirk: without this the next right-click may not show the menu.
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));

    let id = cmd.0 as usize;
    match id {
        ID_QUIT => send(TrayCommand::Quit),
        i if (ID_MODE_BASE..ID_MODE_BASE + MODE_LABELS.len()).contains(&i) => {
            send(TrayCommand::SetMode((i - ID_MODE_BASE) as u8));
        }
        i if (ID_STYLE_BASE..=ID_STYLE_BASE + crate::STYLE_MAX_INDEX as usize).contains(&i) => {
            let style = (i - ID_STYLE_BASE) as u8;
            // Store immediately so the overlay (which polls this atom every frame) switches live.
            if let Some(s) = TRAY_STYLE.get() {
                s.store(style, SeqCst);
            }
            send(TrayCommand::SetStyle(style));
        }
        _ => {}
    }
}
