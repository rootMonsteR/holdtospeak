//! `Win32Hotkey` — the WH_KEYBOARD_LL push-to-talk hook (a `HotkeySource`). Runs the hook +
//! message loop on its own thread and streams `HotkeyEvent`s. Ported verbatim from the validated
//! prototype: our-own-event tagging, stuck-modifier self-heal, the Start-menu mask-key trick, PTT
//! suppression, and the mode-cycle chord. Decoupled from capture — the pipeline drives the ring.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering::SeqCst};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;

use nib_platform::{Binding, HookHealth, HotkeyEvent, HotkeySource};
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::QueryPerformanceCounter;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VIRTUAL_KEY, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU,
    VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage, HHOOK,
    KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
};

use crate::state::{send_vk, CTRL, INJECT_MAGIC};

const MOD_CTRL: u8 = 1;
const MOD_ALT: u8 = 2;
const MOD_SHIFT: u8 = 4;
const MOD_WIN: u8 = 8;
const VK_MASK_DUMMY: u16 = 0xE8; // unassigned VK — the classic "mask key" (same trick as AHK)

static ALT: AtomicBool = AtomicBool::new(false);
static SHIFT: AtomicBool = AtomicBool::new(false);
static WIN: AtomicBool = AtomicBool::new(false);
static PTT_MODS: AtomicU8 = AtomicU8::new(0);
static CYCLE_MODS: AtomicU8 = AtomicU8::new(0);
static CYCLE_KEY: AtomicU32 = AtomicU32::new(0);
static CYCLE_ARMED: AtomicBool = AtomicBool::new(false);
static WIN_LEAKED: AtomicBool = AtomicBool::new(false);
static RECORDING: AtomicBool = AtomicBool::new(false);
static SINK: OnceLock<Sender<HotkeyEvent>> = OnceLock::new();

fn is_ctrl(vk: u16) -> bool {
    vk == VK_LCONTROL.0 || vk == VK_RCONTROL.0 || vk == VK_CONTROL.0
}
fn is_alt(vk: u16) -> bool {
    vk == VK_LMENU.0 || vk == VK_RMENU.0 || vk == VK_MENU.0
}
fn is_shift(vk: u16) -> bool {
    vk == VK_LSHIFT.0 || vk == VK_RSHIFT.0 || vk == VK_SHIFT.0
}
fn is_win(vk: u16) -> bool {
    vk == VK_LWIN.0 || vk == VK_RWIN.0
}
fn mod_bit(vk: u16) -> u8 {
    if is_ctrl(vk) {
        MOD_CTRL
    } else if is_alt(vk) {
        MOD_ALT
    } else if is_shift(vk) {
        MOD_SHIFT
    } else if is_win(vk) {
        MOD_WIN
    } else {
        0
    }
}
fn cur_mods() -> u8 {
    (if CTRL.load(SeqCst) { MOD_CTRL } else { 0 })
        | (if ALT.load(SeqCst) { MOD_ALT } else { 0 })
        | (if SHIFT.load(SeqCst) { MOD_SHIFT } else { 0 })
        | (if WIN.load(SeqCst) { MOD_WIN } else { 0 })
}
fn key_down_async(vk: VIRTUAL_KEY) -> bool {
    (unsafe { GetAsyncKeyState(vk.0 as i32) } as u16) & 0x8000 != 0
}

/// Derive `(modifier_bitmask, main_key_vk)` from a Binding's canonical VK list (`nib-config`
/// emits standard VK codes; classify each as a modifier or the main key). key = 0 if none.
fn binding_mods_key(b: &Binding) -> (u8, u16) {
    let (mut mods, mut key) = (0u8, 0u16);
    for &vk in &b.keys {
        let mb = mod_bit(vk);
        if mb != 0 {
            mods |= mb;
        } else {
            key = vk;
        }
    }
    (mods, key)
}

/// Monotonic high-resolution timestamp (QueryPerformanceCounter ticks) — stamps PTT edges so the
/// pipeline can measure the key-up→text-inserted latency contract.
fn qpc() -> u64 {
    let mut v = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut v);
    }
    v as u64
}

fn emit(ev: HotkeyEvent) {
    if let Some(tx) = SINK.get() {
        let _ = tx.send(ev);
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        // Our own injected paste keys must not feed modifier tracking or the combo logic.
        if kb.dwExtraInfo == INJECT_MAGIC {
            return CallNextHookEx(HHOOK::default(), code, wparam, lparam);
        }
        let vk = kb.vkCode as u16;
        let msg = wparam.0 as u32;
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let mb = mod_bit(vk);
        match mb {
            MOD_CTRL => CTRL.store(down, SeqCst),
            MOD_ALT => ALT.store(down, SeqCst),
            MOD_SHIFT => SHIFT.store(down, SeqCst),
            MOD_WIN => WIN.store(down, SeqCst),
            _ => {}
        }
        // Heal stale-true modifiers (keyups missed on the UAC secure desktop or after an LL-hook
        // timeout). Skip the current event's own group; clears only, never sets.
        if mb != MOD_CTRL
            && CTRL.load(SeqCst)
            && !key_down_async(VK_LCONTROL)
            && !key_down_async(VK_RCONTROL)
        {
            CTRL.store(false, SeqCst);
        }
        if mb != MOD_ALT
            && ALT.load(SeqCst)
            && !key_down_async(VK_LMENU)
            && !key_down_async(VK_RMENU)
        {
            ALT.store(false, SeqCst);
        }
        if mb != MOD_SHIFT
            && SHIFT.load(SeqCst)
            && !key_down_async(VK_LSHIFT)
            && !key_down_async(VK_RSHIFT)
        {
            SHIFT.store(false, SeqCst);
        }
        if mb != MOD_WIN && WIN.load(SeqCst) && !key_down_async(VK_LWIN) && !key_down_async(VK_RWIN)
        {
            WIN.store(false, SeqCst);
        }
        let mods = cur_mods();

        // cycle-mode hotkey (chord): fire once on keydown while its modifiers are held.
        let ck = CYCLE_KEY.load(SeqCst) as u16;
        let cm = CYCLE_MODS.load(SeqCst);
        if ck != 0 && cm != 0 {
            if down && mb == 0 && vk == ck && (mods & cm) == cm {
                if !CYCLE_ARMED.swap(true, SeqCst) {
                    emit(HotkeyEvent::Secondary);
                }
                if WIN_LEAKED.swap(false, SeqCst) {
                    send_vk(VK_MASK_DUMMY, false);
                    send_vk(VK_MASK_DUMMY, true);
                }
                return LRESULT(1); // swallow the cycle key
            }
            if !down && vk == ck {
                CYCLE_ARMED.store(false, SeqCst);
            }
        }

        // push-to-talk: emit PttDown/PttUp on the combo-held edge.
        let pm = PTT_MODS.load(SeqCst);
        let combo = pm != 0 && (mods & pm) == pm;
        if combo && !RECORDING.load(SeqCst) {
            RECORDING.store(true, SeqCst);
            // Mask a leaked Win-down so the eventual Win-up doesn't open the Start menu.
            if WIN_LEAKED.swap(false, SeqCst) {
                send_vk(VK_MASK_DUMMY, false);
                send_vk(VK_MASK_DUMMY, true);
            }
            emit(HotkeyEvent::PttDown { qpc: qpc() });
        } else if !combo && RECORDING.load(SeqCst) {
            RECORDING.store(false, SeqCst);
            emit(HotkeyEvent::PttUp { qpc: qpc() });
        }
        // Track an un-suppressed Win-down (pending Start-menu gesture) for the mask above.
        if mb == MOD_WIN && !(combo && (mb & pm) != 0) {
            WIN_LEAKED.store(down, SeqCst);
        }
        // Suppress the PTT modifier keys while active so they don't leak to the OS.
        if combo && (mb & pm) != 0 {
            return LRESULT(1);
        }
    }
    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

/// The global push-to-talk hotkey source. `start` installs the hook on a dedicated thread.
#[derive(Debug, Default, Clone, Copy)]
pub struct Win32Hotkey;

impl HotkeySource for Win32Hotkey {
    fn start(&self, ptt: Binding, cycle: Option<Binding>, sink: Sender<HotkeyEvent>) {
        let (pm, _) = binding_mods_key(&ptt);
        PTT_MODS.store(pm, SeqCst);
        if let Some(c) = cycle {
            let (cm, ck) = binding_mods_key(&c);
            CYCLE_MODS.store(cm, SeqCst);
            CYCLE_KEY.store(ck as u32, SeqCst);
        }
        let _ = SINK.set(sink);
        std::thread::spawn(|| unsafe {
            let hmod = GetModuleHandleW(None).unwrap();
            let _hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), HINSTANCE(hmod.0), 0)
                .expect("SetWindowsHookExW");
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        });
    }

    fn health(&self) -> HookHealth {
        HookHealth::Healthy
    }
}
