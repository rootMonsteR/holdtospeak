//! Always-on voice-spectrum overlay — a layered, click-through, always-on-top window pinned
//! bottom-center, rendered into a premultiplied-ARGB DIB section via `UpdateLayeredWindow`
//! (no external UI lib).
//!
//! This file is ONLY the Win32 plumbing: window class/creation, the DIB blit, the visibility
//! gating, and the frame loop. Every pixel — the styles, the FFT/band DSP, the animation state
//! and the renderers — lives in the platform-independent `nib-overlay` crate, which is where a
//! Mac backend would plug in.
//!
//! Ported from `spikes/vslice/src/overlay.rs`. The prototype's three couplings to the binary
//! (the shared capture ring, `CURRENT_STYLE`, `CURRENT_MODE`) are replaced by parameters to
//! [`Win32Overlay::spawn`]: a `sampler` closure for the FFT, and `style`/`mode` atoms polled
//! every frame so the tray's live theme switch and the mode label update instantly.
#![allow(unsafe_code)]

use std::sync::atomic::Ordering::SeqCst;
use std::sync::atomic::{AtomicBool, AtomicU8};
use std::sync::Arc;
use std::time::Duration;

use nib_overlay::{
    compute_bands, compute_bars, hann_window, plan_fft, render_frame, Anim, OverlayStyle, FFT_N,
    GAIN, MARGIN_BOTTOM, OH, OW,
};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, GetDC, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetSystemMetrics, LoadCursorW, PeekMessageW,
    RegisterClassW, ShowWindow, TranslateMessage, UpdateLayeredWindow, HMENU, IDC_ARROW, MSG,
    PM_REMOVE, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

/// The always-on voice-spectrum overlay (a ZST; the OS window + render loop live on its thread).
pub struct Win32Overlay;

impl Win32Overlay {
    /// Spawn the overlay on its own thread (layered click-through window, bottom-center).
    /// Fire-and-forget: the thread runs for the process lifetime. The window starts hidden and
    /// is only VISIBLE while `listening` is true (push-to-talk held); idle it hides and skips work.
    /// - `sampler(n)` returns the most recent up-to-`n` mono samples at `sample_rate` (for the FFT).
    /// - `style` (0-based OverlayStyle index) and `mode` (0=Raw..3=Email) are polled EVERY frame so
    ///   the tray's live theme switch + the mode label update instantly.
    /// - `listening` is polled EVERY frame: false→true shows the window (and resets `Anim` so each
    ///   activation replays the boot animation); true→false hides it.
    /// - `enabled` is the user's overlay on/off switch (settings); when false the window stays
    ///   hidden even while listening, so the setting can flip live without restarting.
    pub fn spawn(
        sampler: Box<dyn Fn(usize) -> Vec<f32> + Send>,
        style: Arc<AtomicU8>,
        mode: Arc<AtomicU8>,
        listening: Arc<AtomicBool>,
        enabled: Arc<AtomicBool>,
        sample_rate: u32,
    ) {
        std::thread::spawn(move || unsafe {
            run(sampler, style, mode, listening, enabled, sample_rate)
        });
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wp, lp)
}

unsafe fn run(
    sampler: Box<dyn Fn(usize) -> Vec<f32> + Send>,
    style: Arc<AtomicU8>,
    mode: Arc<AtomicU8>,
    listening: Arc<AtomicBool>,
    enabled: Arc<AtomicBool>,
    sample_rate: u32,
) {
    // The process is per-monitor-DPI-aware (the settings window's WebView2 needs that to render
    // crisply). This overlay is a fixed 460×84 px bitmap drawn for the classic 96-dpi virtual
    // desktop, so this THREAD opts back out: DWM scales the layered window up on HiDPI displays
    // exactly as it did before the process became aware, instead of shrinking it.
    {
        use windows::Win32::UI::HiDpi::{
            SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_UNAWARE_GDISCALED,
        };
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE_GDISCALED);
    }
    let hinst = GetModuleHandleW(None).unwrap();
    let class: Vec<u16> = "NibOverlay\0".encode_utf16().collect();
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: HINSTANCE(hinst.0),
        lpszClassName: PCWSTR(class.as_ptr()),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        ..Default::default()
    };
    RegisterClassW(&wc);
    let ex =
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
    let hwnd = CreateWindowExW(
        ex,
        PCWSTR(class.as_ptr()),
        PCWSTR::null(),
        WS_POPUP,
        0,
        0,
        OW,
        OH,
        HWND::default(),
        HMENU::default(),
        HINSTANCE(hinst.0),
        None,
    )
    .expect("overlay CreateWindowExW");

    // DIB section (top-down 32-bit BGRA, premultiplied)
    let screen_dc = GetDC(None);
    let mem_dc = CreateCompatibleDC(screen_dc);
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: OW,
            biHeight: -OH, // negative = top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
        .expect("overlay CreateDIBSection");
    let _old = SelectObject(mem_dc, HGDIOBJ(dib.0));
    let px = std::slice::from_raw_parts_mut(bits as *mut u32, (OW * OH) as usize);

    let sw = GetSystemMetrics(SM_CXSCREEN);
    let sh = GetSystemMetrics(SM_CYSCREEN);
    let pos = POINT {
        x: (sw - OW) / 2,
        y: sh - OH - MARGIN_BOTTOM,
    };

    let fft = plan_fft();
    let hann = hann_window();
    let mut anim = Anim::new();
    let mut prev_style = OverlayStyle::from_index(style.load(SeqCst));
    let gain = std::env::var("NIB_GAIN")
        .ok()
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(GAIN);

    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let mut visible = false;
    let mut msg = MSG::default();
    loop {
        let frame_t0 = std::time::Instant::now();
        while PeekMessageW(&mut msg, hwnd, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let cur_style = OverlayStyle::from_index(style.load(SeqCst));
        if cur_style != prev_style {
            // Reset the incoming style's state: it was frozen while another style ran, so its
            // clocks/energies are stale (wrong timecode, spurious Volt strike). Rebuilding also
            // replays the boot animation, which reads as intentional on a theme switch.
            anim = Anim::new();
            prev_style = cur_style;
        }
        // Visible only while push-to-talk is held (`listening`) AND the overlay is switched on;
        // idle it hides and skips work.
        let active = listening.load(SeqCst) && enabled.load(SeqCst);
        if active {
            // false->true: a fresh activation. Reset the clocks so each PTT press replays the
            // boot animation rather than resuming a stale timecode from the previous hold.
            if !visible {
                anim = Anim::new();
                prev_style = cur_style;
            }
            let mode_idx = mode.load(SeqCst);
            // Render the live spectrum. The sampler returns the freshest up-to-FFT_N mono
            // samples; a quiet mic yields a low spectrum (idle animation).
            let samples = sampler(FFT_N);
            if cur_style.needs_levels() {
                compute_bars(
                    &samples,
                    sample_rate,
                    fft.as_ref(),
                    &hann,
                    anim.levels_mut(),
                    gain,
                );
            } else {
                compute_bands(
                    &samples,
                    sample_rate,
                    fft.as_ref(),
                    &hann,
                    anim.bands_mut(),
                    gain,
                );
            }
            anim.tick();
            render_frame(px, cur_style, &mut anim, mode_idx);
            let size = SIZE { cx: OW, cy: OH };
            let src = POINT { x: 0, y: 0 };
            let _ = UpdateLayeredWindow(
                hwnd,
                screen_dc,
                Some(&pos),
                Some(&size),
                mem_dc,
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            if !visible {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                visible = true;
            }
        } else if visible {
            // true->false: PTT released. Hide the window; the loop stays alive so the next
            // press shows it again.
            let _ = ShowWindow(hwnd, SW_HIDE);
            visible = false;
        }
        // Compensate for pump/FFT/render time so the animation clock (fixed DT/frame) tracks
        // wall time and matches the headless dump's tempo. Idle frames poll slower to save CPU.
        let budget = Duration::from_millis(if active { 16 } else { 60 });
        std::thread::sleep(budget.saturating_sub(frame_t0.elapsed()));
    }
}
