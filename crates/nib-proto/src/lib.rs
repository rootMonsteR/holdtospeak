//! `nib-proto` — the IPC contract shared by `hookd`, `core`, and `asr-sidecar`.
//!
//! No platform code, no I/O. Pure message types, framing, version negotiation, and the
//! shared-memory audio ring header. See docs/design/01-core-app-design.md §3.
//!
//! Scaffold: the types below are the intended shape from the design doc, pared to a
//! compiling skeleton. Serialization (CBOR via `ciborium`) and the pipe transport land
//! with the W3 "It types" milestone.
#![deny(unsafe_code)]

/// Wire protocol version. Bumped on any breaking change to a message enum.
/// `Hello`/`HelloAck` negotiate the overlap; a partial update (e.g. old hookd, new core)
/// must degrade gracefully, never hard-fault. See design §3 "Versioning".
pub const PROTO_MIN: u8 = 1;
pub const PROTO_MAX: u8 = 1;

/// 8-byte frame header prepended to every message (design §3 "Framing").
/// `u32 len | u16 msg_type | u8 proto_major | u8 flags`.
pub const FRAME_HEADER_LEN: usize = 8;
/// Max single frame. Audio never travels as a frame — it goes through the SHM ring.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

/// Handshake, first frame in both directions.
#[derive(Debug, Clone)]
pub struct Hello {
    pub proto_min: u8,
    pub proto_max: u8,
    pub build: u32,
    pub pid: u32,
}

/// Messages `hookd` -> `core`.
#[derive(Debug, Clone)]
pub enum HookMsg {
    Hello(Hello),
    Armed,
    PttDown {
        qpc: u64,
        combo_id: u16,
        seq: u32,
    },
    PttUp {
        qpc: u64,
        /// Which binding released (see `COMBO_PTT`/`COMBO_CYCLE`) — without it, a cycle-chord
        /// key-up would be indistinguishable from a push-to-talk release on the wire.
        combo_id: u16,
    },
    Cancel {
        reason: u8,
    },
    KeyCaptured {
        vk: u16,
        mods: u16,
    },
    HookLost {
        code: u32,
    },
    HookRestored,
    Heartbeat {
        qpc: u64,
        events_seen: u64,
    },
    /// Forward-compat: an unknown variant tag is preserved, logged, and ignored.
    Unknown {
        tag: u16,
    },
}

/// Messages `core` -> `hookd`.
#[derive(Debug, Clone)]
pub enum CtlMsg {
    SetBinding {
        combo_id: u16,
        arming_ms: u16,
        suppress: bool,
    },
    CaptureMode {
        on: bool,
    },
    Suspend,
    Resume,
    Ping,
    Shutdown,
}

/// Well-known `combo_id`s. The wire carries an id (a `hookd` may host several bindings); the
/// pipeline's `HotkeyEvent` vocabulary distinguishes only push-to-talk from "some secondary
/// binding fired", so this is the agreed mapping between the two.
pub const COMBO_PTT: u16 = 0;
/// The mode-cycle chord — maps to [`nib_platform::HotkeyEvent::Secondary`].
pub const COMBO_CYCLE: u16 = 1;

/// Translate a wire message from `hookd` into the pipeline's platform-level event.
///
/// This exists so the two enums can't silently drift before the process split lands: today the
/// hook and pipeline share a process and pass `HotkeyEvent` directly, but `HookMsg` is what will
/// travel the pipe. Keeping (and testing) the mapping in one place means a change to either
/// vocabulary has an obvious, compiling home. Returns `None` for messages the pipeline doesn't
/// consume (handshake, health, telemetry) — those are handled by the supervisor, not the loop.
pub fn hook_msg_to_event(msg: &HookMsg) -> Option<nib_platform::HotkeyEvent> {
    use nib_platform::HotkeyEvent;
    match *msg {
        HookMsg::Armed => Some(HotkeyEvent::Armed),
        HookMsg::PttDown { qpc, combo_id, .. } => match combo_id {
            COMBO_CYCLE => Some(HotkeyEvent::Secondary),
            _ => Some(HotkeyEvent::PttDown { qpc }),
        },
        // A cycle chord is a one-shot (its Down already emitted `Secondary`); only a
        // push-to-talk release means anything to the pipeline.
        HookMsg::PttUp { qpc, combo_id } => match combo_id {
            COMBO_CYCLE => None,
            _ => Some(HotkeyEvent::PttUp { qpc }),
        },
        HookMsg::Cancel { .. } => Some(HotkeyEvent::Cancel),
        HookMsg::Hello(_)
        | HookMsg::KeyCaptured { .. }
        | HookMsg::HookLost { .. }
        | HookMsg::HookRestored
        | HookMsg::Heartbeat { .. }
        | HookMsg::Unknown { .. } => None,
    }
}

/// The shared-memory audio ring header (design §3 "Audio: shared memory ring").
/// 16 kHz mono f32; SPSC; allocated once per sidecar and reused across sessions.
#[repr(C, align(64))]
#[derive(Debug)]
pub struct RingHeader {
    pub magic: u32,
    pub version: u32,
    pub capacity_frames: u64,
    pub sample_rate: u32,
    pub session_id: u64,
    /// Producer cursor (core `T-capture`). Written with Release ordering.
    pub write_idx: u64,
    /// Consumer cursor (sidecar `T-ring`). Written with Acquire ordering.
    pub read_idx: u64,
    /// Bit 0 = EOS, bit 1 = GLITCH (data discontinuity).
    pub flags: u32,
}

pub const RING_MAGIC: u32 = 0x4E49_4252; // "NIBR"
pub const RING_FLAG_EOS: u32 = 1 << 0;
pub const RING_FLAG_GLITCH: u32 = 1 << 1;

/// True when the two builds share a usable protocol range.
pub fn protocol_compatible(peer: &Hello) -> bool {
    peer.proto_min <= PROTO_MAX && peer.proto_max >= PROTO_MIN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_versions_are_compatible() {
        let peer = Hello {
            proto_min: 1,
            proto_max: 2,
            build: 0,
            pid: 0,
        };
        assert!(protocol_compatible(&peer));
    }

    #[test]
    fn disjoint_versions_are_incompatible() {
        let peer = Hello {
            proto_min: 5,
            proto_max: 7,
            build: 0,
            pid: 0,
        };
        assert!(!protocol_compatible(&peer));
    }

    #[test]
    fn ring_header_is_64_byte_aligned() {
        assert_eq!(std::mem::align_of::<RingHeader>(), 64);
    }

    #[test]
    fn ptt_edges_map_and_preserve_qpc() {
        use nib_platform::HotkeyEvent;
        let down = hook_msg_to_event(&HookMsg::PttDown {
            qpc: 4242,
            combo_id: COMBO_PTT,
            seq: 1,
        });
        assert!(matches!(down, Some(HotkeyEvent::PttDown { qpc: 4242 })));
        let up = hook_msg_to_event(&HookMsg::PttUp {
            qpc: 9001,
            combo_id: COMBO_PTT,
        });
        assert!(matches!(up, Some(HotkeyEvent::PttUp { qpc: 9001 })));
    }

    #[test]
    fn cycle_release_is_not_a_ptt_release() {
        // A cycle chord's key-up must NOT surface as PttUp — it would trigger a spurious
        // end_utterance in the pipeline after the process split.
        let up = hook_msg_to_event(&HookMsg::PttUp {
            qpc: 1,
            combo_id: COMBO_CYCLE,
        });
        assert!(up.is_none());
    }

    #[test]
    fn cycle_combo_maps_to_secondary() {
        use nib_platform::HotkeyEvent;
        let ev = hook_msg_to_event(&HookMsg::PttDown {
            qpc: 1,
            combo_id: COMBO_CYCLE,
            seq: 0,
        });
        assert!(matches!(ev, Some(HotkeyEvent::Secondary)));
    }

    #[test]
    fn supervisor_only_messages_are_not_pipeline_events() {
        for msg in [
            HookMsg::HookLost { code: 1 },
            HookMsg::HookRestored,
            HookMsg::Heartbeat {
                qpc: 0,
                events_seen: 0,
            },
            HookMsg::KeyCaptured { vk: 65, mods: 0 },
            HookMsg::Unknown { tag: 999 },
        ] {
            assert!(hook_msg_to_event(&msg).is_none(), "{msg:?}");
        }
    }
}
