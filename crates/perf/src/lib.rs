//! Looks at memory, swap, disk and the processes a Compose Multiplatform
//! developer's machine spends its memory on. macOS only: the numbers come
//! from `vm_stat`, `sysctl`, `df` and `ps`, run through the `System` trait
//! so everything here can be tested with canned output.

mod classify;
mod parse;

use lf_mide_core::{PerfSample, Pressure, ProcessKind, ProcessStat, SystemStats};
use lf_mide_system::System;

pub use classify::{classify, collect, collect_with_members};
pub use parse::{PsRow, parse_df_free, parse_ps, parse_swap_used, parse_vm_stat_used};

// Absolute paths: the samples run every few seconds and must not depend on
// (or be redirected by) the directories in PATH.
const PS: &str = "/bin/ps";
const SYSCTL: &str = "/usr/sbin/sysctl";
const VM_STAT: &str = "/usr/bin/vm_stat";
const DF: &str = "/bin/df";
const PS_COLUMNS: &str = "pid=,ppid=,rss=,pcpu=,args=";

/// One sample of the machine. The process list is required; a missing
/// machine-wide number just shows as zero.
pub fn sample(sys: &dyn System, own_pid: u32) -> Result<PerfSample, String> {
    let ps = sys
        .run(PS, &["-ww", "-axo", PS_COLUMNS])
        .map_err(|error| format!("ps: {error}"))?;
    if !ps.success {
        return Err("ps failed".to_string());
    }
    let text_of = |program: &str, args: &[&str]| {
        sys.run(program, args)
            .ok()
            .filter(|out| out.success)
            .map(|out| out.stdout)
    };
    let number_of = |program: &str, args: &[&str]| {
        text_of(program, args)
            .and_then(|text| text.trim().parse::<u64>().ok())
            .unwrap_or(0)
    };
    let pressure_level = number_of(SYSCTL, &["-n", "kern.memorystatus_vm_pressure_level"]);
    let system = SystemStats {
        ram_total: number_of(SYSCTL, &["-n", "hw.memsize"]),
        ram_used: text_of(VM_STAT, &[])
            .and_then(|text| parse_vm_stat_used(&text))
            .unwrap_or(0),
        swap_used: text_of(SYSCTL, &["-n", "vm.swapusage"])
            .and_then(|text| parse_swap_used(&text))
            .unwrap_or(0),
        pressure: Pressure::from_level(u32::try_from(pressure_level).unwrap_or(u32::MAX)),
        disk_free: text_of(DF, &["-k", "/"])
            .and_then(|text| parse_df_free(&text))
            .unwrap_or(0),
    };
    let mut processes: Vec<ProcessStat> = collect_with_members(&parse_ps(&ps.stdout), own_pid)
        .into_iter()
        .map(|(stat, members)| ProcessStat {
            rss: footprint_of(sys, &members),
            ..stat
        })
        .collect();
    processes.sort_by(|a, b| b.rss.cmp(&a.rss).then(a.pid.cmp(&b.pid)));
    Ok(PerfSample { system, processes })
}

/// What the processes really hold: each one's footprint where macOS tells
/// us, its resident size otherwise. Resident size counts pages shared with
/// other processes again for every process, which overstates a simulator
/// (dozens of processes) several times over.
fn footprint_of(sys: &dyn System, members: &[(u32, u64)]) -> u64 {
    members.iter().fold(0u64, |sum, (pid, resident)| {
        sum.saturating_add(sys.footprint(*pid).unwrap_or(*resident))
    })
}

/// True when `pid` is still the kind of process we think it is. The list we
/// show is a few seconds old, and process ids get reused.
pub fn is_still(sys: &dyn System, pid: u32, kind: ProcessKind, own_pid: u32) -> bool {
    if pid == own_pid {
        return false;
    }
    let Ok(out) = sys.run(PS, &["-ww", "-p", &pid.to_string(), "-o", "args="]) else {
        return false;
    };
    out.success && classify(out.stdout.trim(), pid, own_pid).is_some_and(|(found, _)| found == kind)
}

#[cfg(test)]
mod tests {
    use lf_mide_system::fake::FakeSystem;

    use super::*;

    const PS_TEXT: &str = include_str!("../fixtures/ps.txt");
    const VM_STAT_TEXT: &str = include_str!("../fixtures/vm_stat.txt");
    const GIB: u64 = 1024 * 1024 * 1024;

    fn machine() -> FakeSystem {
        FakeSystem::new()
            .ok("/bin/ps -ww -axo pid=,ppid=,rss=,pcpu=,args=", PS_TEXT)
            .ok("/usr/sbin/sysctl -n hw.memsize", "17179869184\n")
            .ok(
                "/usr/sbin/sysctl -n vm.swapusage",
                include_str!("../fixtures/swapusage.txt"),
            )
            .ok(
                "/usr/sbin/sysctl -n kern.memorystatus_vm_pressure_level",
                "2\n",
            )
            .ok("/usr/bin/vm_stat", VM_STAT_TEXT)
            .ok("/bin/df -k /", include_str!("../fixtures/df.txt"))
    }

    #[test]
    fn a_sample_combines_the_machine_numbers_and_the_processes() {
        let taken = sample(&machine(), 3790).unwrap();

        assert_eq!(taken.system.ram_total, 16 * GIB);
        assert_eq!(taken.system.pressure, Pressure::Warn);
        assert!(taken.system.swap_used > 2 * GIB && taken.system.swap_used < 3 * GIB);
        assert!(taken.system.disk_free > 48 * GIB);
        assert!(taken.system.ram_used > 10 * GIB && taken.system.ram_used < 16 * GIB);
        assert!(taken.processes.len() >= 8);
    }

    #[test]
    fn footprints_replace_resident_sizes_and_the_list_is_sorted_again() {
        const MIB: u64 = 1024 * 1024;
        let sys = machine()
            // The simulator: two of its four processes are known.
            .footprint_of(48452, 10 * MIB)
            .footprint_of(1407, 20 * MIB)
            // The Gradle daemon holds far less than the old list suggested.
            .footprint_of(4121, 100 * MIB);

        let taken = sample(&sys, 3790).unwrap();

        let sim = taken
            .processes
            .iter()
            .find(|p| p.kind == ProcessKind::Simulator)
            .unwrap();
        assert_eq!(sim.rss, (10 + 20) * MIB + (80_256 + 55_552) * 1024);
        let gradle = taken
            .processes
            .iter()
            .find(|p| p.kind == ProcessKind::GradleDaemon)
            .unwrap();
        assert_eq!(gradle.rss, 100 * MIB);
        assert!(taken.processes.windows(2).all(|w| w[0].rss >= w[1].rss));
        let kotlin = taken
            .processes
            .iter()
            .find(|p| p.kind == ProcessKind::KotlinDaemon)
            .unwrap();
        assert_eq!(
            kotlin.rss,
            942_080 * 1024,
            "unknown footprint falls back to resident size"
        );
    }

    #[test]
    fn missing_machine_numbers_become_zero_but_a_missing_ps_is_an_error() {
        let only_ps = FakeSystem::new().ok("/bin/ps -ww -axo pid=,ppid=,rss=,pcpu=,args=", PS_TEXT);

        let taken = sample(&only_ps, 1).unwrap();

        assert_eq!(taken.system, SystemStats::default());
        assert!(sample(&FakeSystem::new(), 1).is_err());
    }

    #[test]
    fn a_process_is_only_confirmed_if_it_is_still_the_same_kind() {
        let sys = FakeSystem::new()
            .ok(
                "/bin/ps -ww -p 4121 -o args=",
                "/usr/bin/java -cp x org.gradle.launcher.daemon.bootstrap.GradleDaemon 8.14\\n",
            )
            .ok("/bin/ps -ww -p 77 -o args=", "/usr/bin/vim notes.txt\\n")
            .fails("/bin/ps -ww -p 99 -o args=", "");

        assert!(is_still(&sys, 4121, ProcessKind::GradleDaemon, 1));
        assert!(!is_still(&sys, 4121, ProcessKind::KotlinDaemon, 1));
        assert!(
            !is_still(&sys, 77, ProcessKind::GradleDaemon, 1),
            "pid reused"
        );
        assert!(!is_still(&sys, 99, ProcessKind::GradleDaemon, 1), "gone");
        assert!(
            !is_still(&sys, 1, ProcessKind::LfMide, 1),
            "never ourselves"
        );
    }
}
