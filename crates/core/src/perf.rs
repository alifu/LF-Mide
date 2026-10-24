//! Performance status: system memory and the dev processes that use it.
//! Plain data and the rules for judging it; sampling lives in `lf-mide-perf`.

const KIB: u64 = 1024;
const MIB: u64 = KIB * 1024;
const GIB: u64 = MIB * 1024;
/// A swap change smaller than this is not a trend.
pub const SWAP_TREND_MIN_BYTES: u64 = 64 * MIB;

/// How hard macOS says memory is squeezed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Pressure {
    #[default]
    Normal,
    Warn,
    Critical,
}

impl Pressure {
    /// From `kern.memorystatus_vm_pressure_level` (1 normal, 2 warn, 4 critical).
    pub fn from_level(level: u32) -> Pressure {
        match level {
            0 | 1 => Pressure::Normal,
            2 | 3 => Pressure::Warn,
            _ => Pressure::Critical,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Pressure::Normal => "normal",
            Pressure::Warn => "warn",
            Pressure::Critical => "critical",
        }
    }
}

/// Machine-wide numbers, all in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SystemStats {
    pub ram_used: u64,
    pub ram_total: u64,
    pub swap_used: u64,
    pub pressure: Pressure,
    pub disk_free: u64,
}

/// The kinds of process worth watching, by what they do for the developer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProcessKind {
    GradleDaemon,
    KotlinDaemon,
    KotlinLsp,
    /// The Kotlin language server LF-Mide sets up in its Neovim.
    KmpLsp,
    Emulator,
    Simulator,
    Neovim,
    LfMide,
    /// Heavy IDEs the user may have left open.
    AndroidStudio,
    Xcode,
}

impl ProcessKind {
    pub fn label(self) -> &'static str {
        match self {
            ProcessKind::GradleDaemon => "Gradle daemon",
            ProcessKind::KotlinDaemon => "Kotlin compile daemon",
            ProcessKind::KotlinLsp => "kotlin-lsp",
            ProcessKind::KmpLsp => "kmp-lsp",
            ProcessKind::Emulator => "Android emulator",
            ProcessKind::Simulator => "iOS simulator",
            ProcessKind::Neovim => "nvim",
            ProcessKind::LfMide => "lf-mide",
            ProcessKind::AndroidStudio => "Android Studio",
            ProcessKind::Xcode => "Xcode",
        }
    }

    /// LF-Mide may ask these to stop. Editors and IDEs hold the user's
    /// unsaved work, and LF-Mide itself is not for killing.
    pub fn stoppable(self) -> bool {
        matches!(
            self,
            ProcessKind::GradleDaemon
                | ProcessKind::KotlinDaemon
                | ProcessKind::KotlinLsp
                | ProcessKind::KmpLsp
                | ProcessKind::Emulator
                | ProcessKind::Simulator
        )
    }
}

/// One watched process (for the simulator: the whole simulated device).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessStat {
    pub kind: ProcessKind,
    pub pid: u32,
    /// Resident memory in bytes.
    pub rss: u64,
    /// CPU use in tenths of a percent of one core.
    pub cpu_tenths: u32,
    /// Which AVD (emulator) or UDID (simulator); empty otherwise.
    pub detail: String,
}

/// A stop the user has asked for once and must confirm. It names what they
/// saw (kind and detail, not only the pid), so a reused pid cannot inherit it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopRequest {
    pub pid: u32,
    pub kind: ProcessKind,
    pub detail: String,
}

impl StopRequest {
    pub fn of(process: &ProcessStat) -> Self {
        Self {
            pid: process.pid,
            kind: process.kind,
            detail: process.detail.clone(),
        }
    }

    pub fn is_for(&self, process: &ProcessStat) -> bool {
        *self == Self::of(process)
    }
}

/// One look at the machine.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PerfSample {
    pub system: SystemStats,
    /// Biggest memory use first.
    pub processes: Vec<ProcessStat>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Health {
    Good,
    Warn,
    Critical,
}

/// Swap sizes (bytes) at which the indicator changes colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapLimits {
    pub warn: u64,
    pub critical: u64,
}

impl SwapLimits {
    pub fn from_mib(warn: u64, critical: u64) -> Self {
        Self {
            warn: warn.saturating_mul(MIB),
            critical: critical.saturating_mul(MIB),
        }
    }
}

impl SystemStats {
    /// The worse of what the OS says (pressure) and what swap says.
    pub fn health(&self, limits: SwapLimits) -> Health {
        if self.pressure == Pressure::Critical || self.swap_used >= limits.critical {
            Health::Critical
        } else if self.pressure == Pressure::Warn || self.swap_used >= limits.warn {
            Health::Warn
        } else {
            Health::Good
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Trend {
    Up,
    Down,
    #[default]
    Flat,
}

/// Whether swap grew or shrank by a meaningful amount.
pub fn swap_trend(before: u64, now: u64) -> Trend {
    if now.saturating_sub(before) >= SWAP_TREND_MIN_BYTES {
        Trend::Up
    } else if before.saturating_sub(now) >= SWAP_TREND_MIN_BYTES {
        Trend::Down
    } else {
        Trend::Flat
    }
}

/// `1.8G`, `512M`, `48M`, `3K`.
pub fn format_size(bytes: u64) -> String {
    if bytes >= GIB {
        format!("{:.1}G", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{}M", bytes / MIB)
    } else {
        format!("{}K", bytes / KIB)
    }
}

/// `12.3%` for tenths of a percent.
pub fn format_cpu(tenths: u32) -> String {
    format!("{}.{}%", tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: SwapLimits = SwapLimits {
        warn: 2 * GIB,
        critical: 4 * GIB,
    };

    fn stats(swap: u64, pressure: Pressure) -> SystemStats {
        SystemStats {
            swap_used: swap,
            pressure,
            ..SystemStats::default()
        }
    }

    #[test]
    fn pressure_levels_follow_the_kernel_numbers() {
        assert_eq!(Pressure::from_level(1), Pressure::Normal);
        assert_eq!(Pressure::from_level(2), Pressure::Warn);
        assert_eq!(Pressure::from_level(4), Pressure::Critical);
        assert_eq!(Pressure::from_level(0), Pressure::Normal);
        assert_eq!(Pressure::from_level(99), Pressure::Critical);
    }

    #[test]
    fn health_is_the_worse_of_pressure_and_swap() {
        assert_eq!(stats(0, Pressure::Normal).health(LIMITS), Health::Good);
        assert_eq!(stats(GIB, Pressure::Normal).health(LIMITS), Health::Good);
        assert_eq!(
            stats(2 * GIB, Pressure::Normal).health(LIMITS),
            Health::Warn
        );
        assert_eq!(stats(0, Pressure::Warn).health(LIMITS), Health::Warn);
        assert_eq!(
            stats(4 * GIB, Pressure::Normal).health(LIMITS),
            Health::Critical
        );
        assert_eq!(
            stats(0, Pressure::Critical).health(LIMITS),
            Health::Critical
        );
        assert_eq!(
            stats(3 * GIB, Pressure::Critical).health(LIMITS),
            Health::Critical
        );
    }

    #[test]
    fn limits_come_from_mebibytes_without_overflow() {
        assert_eq!(SwapLimits::from_mib(2048, 4096), LIMITS);
        assert_eq!(SwapLimits::from_mib(u64::MAX, 1).warn, u64::MAX);
    }

    #[test]
    fn small_swap_changes_are_not_a_trend() {
        assert_eq!(swap_trend(GIB, GIB + 10 * MIB), Trend::Flat);
        assert_eq!(swap_trend(GIB, GIB + 64 * MIB), Trend::Up);
        assert_eq!(swap_trend(GIB, GIB - 64 * MIB), Trend::Down);
        assert_eq!(swap_trend(0, 0), Trend::Flat);
        assert_eq!(swap_trend(10, 0), Trend::Flat);
    }

    #[test]
    fn trends_survive_extreme_values() {
        assert_eq!(swap_trend(u64::MAX, u64::MAX), Trend::Flat);
        assert_eq!(swap_trend(u64::MAX, 0), Trend::Down);
        assert_eq!(swap_trend(0, u64::MAX), Trend::Up);
    }

    #[test]
    fn sizes_are_short_and_readable() {
        assert_eq!(format_size(1_932_735_283), "1.8G");
        assert_eq!(format_size(512 * MIB), "512M");
        assert_eq!(format_size(3 * KIB + 5), "3K");
        assert_eq!(format_size(0), "0K");
        assert_eq!(format_cpu(123), "12.3%");
        assert_eq!(format_cpu(5), "0.5%");
    }

    #[test]
    fn only_background_helpers_can_be_stopped() {
        for kind in [
            ProcessKind::GradleDaemon,
            ProcessKind::KotlinDaemon,
            ProcessKind::KotlinLsp,
            ProcessKind::KmpLsp,
            ProcessKind::Emulator,
            ProcessKind::Simulator,
        ] {
            assert!(kind.stoppable(), "{kind:?}");
        }
        for kind in [
            ProcessKind::Neovim,
            ProcessKind::LfMide,
            ProcessKind::AndroidStudio,
            ProcessKind::Xcode,
        ] {
            assert!(!kind.stoppable(), "{kind:?}");
        }
    }
}
