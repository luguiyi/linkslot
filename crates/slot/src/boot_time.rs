use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

/// Milliseconds since the frontend process started.  This is the number that
/// matters for app work, but it is not comparable with the launcher's lines.
pub fn ms() -> f64 {
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1e3
}

/// Seconds since kernel boot.  Printed with every mark so the frontend stages
/// can be lined up with the shell launcher's `slot-startup-r2.log` entries.
fn uptime_s() -> f64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(text) = std::fs::read_to_string("/proc/uptime") {
            if let Some(value) = text.split_whitespace().next() {
                if let Ok(seconds) = value.parse::<f64>() {
                    return seconds;
                }
            }
        }
    }
    0.0
}

pub fn mark(phase: &str) {
    eprintln!(
        "slot: boot: {phase} at {:.1} ms (uptime {:.2}s)",
        ms(),
        uptime_s()
    );
}
