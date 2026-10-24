//! Reading the output of `vm_stat`, `sysctl`, `df` and `ps`.

const KIB: u64 = 1024;
const MIB: u64 = KIB * 1024;
const GIB: u64 = MIB * 1024;

/// RAM in use, counted like Activity Monitor: app memory (anonymous pages
/// that are not purgeable) + wired + compressed.
pub fn parse_vm_stat_used(text: &str) -> Option<u64> {
    let page_size = text
        .lines()
        .next()?
        .split("page size of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    let pages = |name: &str| -> Option<u64> {
        let line = text.lines().find(|l| l.starts_with(name))?;
        line.split(':')
            .nth(1)?
            .trim()
            .trim_end_matches('.')
            .parse()
            .ok()
    };
    let app = pages("Anonymous pages")?.saturating_sub(pages("Pages purgeable").unwrap_or(0));
    let wired = pages("Pages wired down").unwrap_or(0);
    let compressed = pages("Pages occupied by compressor").unwrap_or(0);
    Some(
        app.saturating_add(wired)
            .saturating_add(compressed)
            .saturating_mul(page_size),
    )
}

/// The `used` figure of `vm.swapusage` (`used = 2088.12M`).
pub fn parse_swap_used(text: &str) -> Option<u64> {
    let value = text.split("used =").nth(1)?.split_whitespace().next()?;
    let (number, unit) = value.split_at(value.len().checked_sub(1)?);
    // A comma-decimal locale prints `1920,12M`.
    let number: f64 = number.replace(',', ".").parse().ok()?;
    if !number.is_finite() {
        return None;
    }
    let scale = match unit {
        "K" => KIB,
        "M" => MIB,
        "G" => GIB,
        _ => return None,
    };
    (number >= 0.0).then_some((number * scale as f64) as u64)
}

/// Free space in bytes from `df -k /` (the "Available" column).
pub fn parse_df_free(text: &str) -> Option<u64> {
    let row = text.lines().nth(1)?;
    let available: u64 = row.split_whitespace().nth(3)?.parse().ok()?;
    available.checked_mul(KIB)
}

/// One line of `ps -axo pid=,ppid=,rss=,pcpu=,args=`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsRow {
    pub pid: u32,
    pub ppid: u32,
    /// Resident memory in bytes.
    pub rss: u64,
    pub cpu_tenths: u32,
    pub args: String,
}

pub fn parse_ps(text: &str) -> Vec<PsRow> {
    text.lines().filter_map(parse_ps_line).collect()
}

fn parse_ps_line(line: &str) -> Option<PsRow> {
    let mut rest = line.trim_start();
    let mut field = || {
        let end = rest.find(char::is_whitespace)?;
        let (token, tail) = rest.split_at(end);
        rest = tail.trim_start();
        Some(token)
    };
    let pid = field()?.parse().ok()?;
    let ppid = field()?.parse().ok()?;
    let rss_kib: u64 = field()?.parse().ok()?;
    let cpu: f64 = field()?.replace(',', ".").parse().ok()?;
    Some(PsRow {
        pid,
        ppid,
        rss: rss_kib.saturating_mul(KIB),
        cpu_tenths: if cpu.is_finite() && cpu > 0.0 {
            (cpu * 10.0).round() as u32
        } else {
            0
        },
        args: rest.trim_end().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ram_used_is_app_plus_wired_plus_compressed_in_pages_of_the_reported_size() {
        let used = parse_vm_stat_used(include_str!("../fixtures/vm_stat.txt")).unwrap();

        // (409253 - 9881 + 176797 + 213074) pages of 16384 bytes
        assert_eq!(used, (409_253 - 9_881 + 176_797 + 213_074) * 16_384);
    }

    #[test]
    fn vm_stat_without_the_needed_lines_is_not_guessed_at() {
        assert_eq!(parse_vm_stat_used(""), None);
        assert_eq!(
            parse_vm_stat_used("Mach Virtual Memory Statistics: (page size of 4096 bytes)\n"),
            None
        );
        assert_eq!(parse_vm_stat_used("garbage"), None);
    }

    #[test]
    fn swap_used_understands_the_unit() {
        let used = parse_swap_used(include_str!("../fixtures/swapusage.txt")).unwrap();

        assert_eq!(used, (2088.12 * 1024.0 * 1024.0) as u64);
        assert_eq!(
            parse_swap_used("total = 1G  used = 1.5G  free = 0G"),
            Some(1_610_612_736)
        );
        assert_eq!(
            parse_swap_used("total = 0K  used = 0.00K  free = 0K"),
            Some(0)
        );
        assert_eq!(parse_swap_used("used = 12X"), None);
        assert_eq!(parse_swap_used("used = "), None);
        assert_eq!(parse_swap_used(""), None);
    }

    #[test]
    fn a_comma_decimal_locale_is_understood() {
        assert_eq!(
            parse_swap_used("total = 3072,00M  used = 1920,12M  free = 1M"),
            Some((1920.12 * 1024.0 * 1024.0) as u64)
        );
        let rows = parse_ps("  7 1 100 12,3 /bin/x\n");
        assert_eq!(rows[0].cpu_tenths, 123);
    }

    #[test]
    fn absurd_swap_numbers_are_refused_not_saturated() {
        assert_eq!(parse_swap_used("used = infG"), None);
        assert_eq!(parse_swap_used("used = NaNM"), None);
        assert_eq!(parse_swap_used("used = 1e300G"), Some(u64::MAX));
    }

    #[test]
    fn disk_free_is_the_available_column_in_bytes() {
        let free = parse_df_free(include_str!("../fixtures/df.txt")).unwrap();

        assert_eq!(free, 51_364_260 * 1024);
        assert_eq!(parse_df_free("Filesystem only a header"), None);
        assert_eq!(parse_df_free(""), None);
    }

    #[test]
    fn ps_rows_keep_the_whole_command_line() {
        let rows = parse_ps(include_str!("../fixtures/ps.txt"));

        let sim = rows.iter().find(|r| r.pid == 48452).unwrap();
        assert_eq!((sim.ppid, sim.rss, sim.cpu_tenths), (1, 13_616 * 1024, 0));
        assert!(sim.args.starts_with("launchd_sim /Users/me/Library"));
        let java = rows.iter().find(|r| r.pid == 4121).unwrap();
        assert_eq!(java.cpu_tenths, 32);
        assert!(java.args.contains("-Xmx2g") && java.args.ends_with("GradleDaemon 8.14"));
    }

    #[test]
    fn broken_ps_lines_are_skipped() {
        let rows = parse_ps(
            "not a row\n  12 1 100 0.0 /bin/ok\n  x 1 2 3 /bad\n\n  13 1 abc 0.0 /bad2\n  14 1 5 nan /nan\n",
        );

        let pids: Vec<u32> = rows.iter().map(|r| r.pid).collect();
        assert_eq!(pids, [12, 14], "a cpu that is not a number counts as zero");
    }

    #[test]
    fn huge_numbers_do_not_overflow() {
        let rows = parse_ps("  1 0 18446744073709551615 0.0 /big\n");

        assert_eq!(rows[0].rss, u64::MAX);
    }
}
