//! Free disk space and RAM.

use lf_mide_core::{Category, CheckResult, Status};

use super::{CheckFn, Ctx};

const KB: u64 = 1024;
const GB: u64 = 1024 * 1024 * 1024;
const LOW_DISK_BYTES: u64 = 20 * GB;
const CRITICAL_DISK_BYTES: u64 = 5 * GB;
const LOW_RAM_BYTES: u64 = 8 * GB;

pub const CHECKS: &[CheckFn] = &[disk, ram];

/// Available bytes from `df -k <path>` output (4th column of the data row).
pub fn parse_df_available(df_output: &str) -> Option<u64> {
    let row = df_output.lines().nth(1)?;
    let available_kb: u64 = row.split_whitespace().nth(3)?.parse().ok()?;
    available_kb.checked_mul(KB)
}

/// Installed RAM in bytes, from `sysctl -n hw.memsize`.
pub fn ram_bytes(ctx: &Ctx) -> Option<u64> {
    let output = ctx.sys.run("sysctl", &["-n", "hw.memsize"]).ok()?;
    output.stdout.trim().parse().ok()
}

pub fn format_gb(bytes: u64) -> String {
    let gb = bytes as f64 / GB as f64;
    if (gb - gb.round()).abs() < 0.05 {
        format!("{gb:.0} GB")
    } else {
        format!("{gb:.1} GB")
    }
}

fn disk(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "Free disk space";
    let available = ctx
        .sys
        .run("df", &["-k", "/"])
        .ok()
        .and_then(|output| parse_df_available(&output.stdout));
    let result = match available {
        None => CheckResult::new(
            Category::Resources,
            NAME,
            Status::Warn,
            "could not read free space",
        ),
        Some(bytes) => {
            let status = match bytes {
                b if b < CRITICAL_DISK_BYTES => Status::Missing,
                b if b < LOW_DISK_BYTES => Status::Warn,
                _ => Status::Ok,
            };
            let result = CheckResult::new(
                Category::Resources,
                NAME,
                status,
                format!("{} free", format_gb(bytes)),
            );
            if status == Status::Ok {
                result
            } else {
                result.with_hint(
                    "Run `lf-mide clean` to see what can be freed (DerivedData, old Gradle versions, unused simulator runtimes)",
                )
            }
        }
    };
    vec![result]
}

fn ram(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "RAM";
    let result = match ram_bytes(ctx) {
        None => CheckResult::new(
            Category::Resources,
            NAME,
            Status::Warn,
            "could not read RAM size",
        ),
        Some(bytes) if bytes < LOW_RAM_BYTES => {
            CheckResult::new(Category::Resources, NAME, Status::Warn, format_gb(bytes))
                .with_hint("Keep Gradle/Kotlin daemon heaps small and run one emulator at a time")
        }
        Some(bytes) => CheckResult::new(Category::Resources, NAME, Status::Ok, format_gb(bytes)),
    };
    vec![result]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeSystem;

    const DF: &str = "Filesystem   1024-blocks      Used Available Capacity iused ifree %iused  Mounted on\n/dev/disk3s1s1 482797652 12582912 59768832    18%  459k 600M    0%   /\n";

    fn run(check: CheckFn, sys: &FakeSystem) -> CheckResult {
        check(&Ctx { sys, project: None }).remove(0)
    }

    #[test]
    fn parses_available_kilobytes_from_df() {
        assert_eq!(parse_df_available(DF), Some(59_768_832 * KB));
        assert_eq!(parse_df_available("garbage"), None);
        assert_eq!(parse_df_available(""), None);
    }

    #[test]
    fn formats_gigabytes_with_one_decimal_below_100() {
        assert_eq!(format_gb(16 * GB), "16 GB");
        assert_eq!(format_gb(GB + GB / 2), "1.5 GB");
        assert_eq!(format_gb(57 * GB), "57 GB");
    }

    #[test]
    fn ram_is_read_from_sysctl() {
        let sys = FakeSystem::new().ok("sysctl -n hw.memsize", "17179869184\n");

        assert_eq!(
            ram_bytes(&Ctx {
                sys: &sys,
                project: None
            }),
            Some(16 * GB)
        );
        assert_eq!(
            ram_bytes(&Ctx {
                sys: &FakeSystem::new(),
                project: None
            }),
            None
        );
    }

    #[test]
    fn disk_status_follows_thresholds() {
        let plenty = run(disk, &FakeSystem::new().ok("df -k /", DF));
        let low = run(
            disk,
            &FakeSystem::new().ok(
                "df -k /",
                &DF.replace("59768832", &(10 * GB / KB).to_string()),
            ),
        );
        let critical = run(
            disk,
            &FakeSystem::new().ok(
                "df -k /",
                &DF.replace("59768832", &(2 * GB / KB).to_string()),
            ),
        );

        assert_eq!(
            (plenty.status, low.status, critical.status),
            (Status::Ok, Status::Warn, Status::Missing)
        );
        assert!(plenty.detail.contains("57 GB free"), "{}", plenty.detail);
        assert!(low.hint.is_some());
    }

    #[test]
    fn disk_warns_when_df_is_unavailable() {
        assert_eq!(run(disk, &FakeSystem::new()).status, Status::Warn);
    }

    #[test]
    fn ram_status_follows_threshold() {
        let ok = run(
            ram,
            &FakeSystem::new().ok("sysctl -n hw.memsize", &(16 * GB).to_string()),
        );
        let low = run(
            ram,
            &FakeSystem::new().ok("sysctl -n hw.memsize", &(4 * GB).to_string()),
        );

        assert_eq!((ok.status, ok.detail.as_str()), (Status::Ok, "16 GB"));
        assert_eq!(low.status, Status::Warn);
    }
}
