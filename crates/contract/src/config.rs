//! Every timeout and limit in one place (spec §5). Transport timeouts stay
//! ABOVE the session machine's own timers so the machine, not the transport,
//! decides what the user sees.

use std::time::Duration;

pub const AUDIO_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
pub const STT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const STT_FINALIZE_TIMEOUT: Duration = Duration::from_secs(5);
pub const STT_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(8);
pub const LLM_FIRST_TOKEN_TIMEOUT: Duration = Duration::from_secs(10);
pub const LLM_TOTAL_TIMEOUT: Duration = Duration::from_secs(60);
pub const RECORD_CAP: Duration = Duration::from_secs(120);
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// Commands that need the core wait at most this long while it is starting.
pub const CORE_READY_WAIT: Duration = Duration::from_secs(25);
/// `get_status` never blocks longer than this on a busy core.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(2);
/// Exit: cancel session, close clients, stop audio worker — then force exit.
pub const SHUTDOWN_BUDGET: Duration = Duration::from_secs(3);
/// A second close within this window always closes.
pub const CLOSE_GUARD_WINDOW: Duration = Duration::from_secs(10);

pub const TRANSPORT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const TRANSPORT_READ_TIMEOUT: Duration = Duration::from_secs(75);
pub const TRANSPORT_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
pub const TRANSPORT_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(10);

pub const PREWARM_THROTTLE: Duration = Duration::from_secs(2);
pub const PREWARM_TIMEOUT: Duration = Duration::from_secs(3);

/// 16 kHz mono i16, 2048-sample (128 ms) frames.
pub const SAMPLE_RATE: u32 = 16_000;
pub const FRAME_SAMPLES: usize = 2048;
/// Frames captured before the STT socket opens are buffered (drop oldest).
pub const PRE_CONNECT_BUFFER_FRAMES: usize = 120;

pub const MAX_TOKENS: u32 = 1024;
/// Provider error snippets quoted to the user are cut at this many chars.
pub const PROVIDER_SNIPPET_CHARS: usize = 200;

/// Session-timer bundle so tests can shrink or inspect them. `Default` is the
/// production values above.
#[derive(Debug, Clone)]
pub struct SessionTimeouts {
    pub audio_drain: Duration,
    pub stt_connect: Duration,
    pub stt_finalize: Duration,
    pub llm_first_token: Duration,
    pub llm_total: Duration,
    pub record_cap: Duration,
}

impl Default for SessionTimeouts {
    fn default() -> Self {
        Self {
            audio_drain: AUDIO_DRAIN_TIMEOUT,
            stt_connect: STT_CONNECT_TIMEOUT,
            stt_finalize: STT_FINALIZE_TIMEOUT,
            llm_first_token: LLM_FIRST_TOKEN_TIMEOUT,
            llm_total: LLM_TOTAL_TIMEOUT,
            record_cap: RECORD_CAP,
        }
    }
}
