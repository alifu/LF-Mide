//! Runs the sampler against this machine. Read-only; run on demand:
//! `cargo test -p lf-mide-perf -- --ignored --nocapture`.

use lf_mide_core::{ProcessKind, format_size};
use lf_mide_system::SystemRunner;

#[test]
#[ignore = "inspects the real machine"]
fn the_real_machine_gives_believable_numbers() {
    let sample = lf_mide_perf::sample(&SystemRunner::new(), std::process::id()).unwrap();

    let s = sample.system;
    println!(
        "RAM {}/{} swap {} pressure {} disk free {}",
        format_size(s.ram_used),
        format_size(s.ram_total),
        format_size(s.swap_used),
        s.pressure.label(),
        format_size(s.disk_free)
    );
    for p in &sample.processes {
        println!(
            "{:<24} pid {:>6} {:>7} {}",
            p.kind.label(),
            p.pid,
            format_size(p.rss),
            p.detail
        );
    }

    assert!(s.ram_total >= 4 * 1024 * 1024 * 1024);
    assert!(s.ram_used > 0 && s.ram_used < s.ram_total);
    assert!(s.disk_free > 0);
    assert!(
        sample
            .processes
            .iter()
            .any(|p| p.kind == ProcessKind::LfMide)
    );
}
