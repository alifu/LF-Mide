//! Quitting must stop commands still being waited for. This lives in its own
//! test binary because the list of running commands is process-wide: other
//! tests running commands at the same moment would be stopped too.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use lf_mide_system::{
    RunError, System, SystemRunner, allow_commands_again, running_commands,
    terminate_running_commands,
};

/// The list is process-wide, so these tests take turns.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

#[test]
fn a_command_still_running_at_quit_is_stopped_and_the_wait_ends() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    allow_commands_again();
    assert_eq!(running_commands(), 0);
    let waiting = std::thread::spawn(|| {
        SystemRunner::with_timeout(Duration::from_secs(60)).run("sh", &["-c", "sleep 30"])
    });
    let started = Instant::now();
    while running_commands() == 0 {
        assert!(started.elapsed() < Duration::from_secs(5), "never started");
        std::thread::sleep(Duration::from_millis(20));
    }

    terminate_running_commands();

    let result = waiting.join().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the wait did not end"
    );
    assert!(!result.expect("the run returned its output").success);
    assert_eq!(running_commands(), 0, "nothing stays listed");
}

#[test]
fn a_command_that_ignores_the_polite_request_is_forced() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    allow_commands_again();
    let waiting = std::thread::spawn(|| {
        SystemRunner::with_timeout(Duration::from_secs(60))
            .run("sh", &["-c", "trap '' TERM; while true; do sleep 1; done"])
    });
    let started = Instant::now();
    while running_commands() == 0 {
        assert!(started.elapsed() < Duration::from_secs(5), "never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(300));

    terminate_running_commands();

    let result = waiting.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(!result.unwrap().success);
}

#[test]
fn after_the_sweep_no_new_command_can_start() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    allow_commands_again();
    terminate_running_commands();

    let late = SystemRunner::new().run("sh", &["-c", "sleep 30"]);
    let late_in = SystemRunner::new().run_in(std::path::Path::new("/"), "sh", &["-c", "sleep 30"]);

    assert!(matches!(late, Err(RunError::Other(m)) if m.contains("shutting down")));
    assert!(late_in.is_err());
    assert_eq!(running_commands(), 0);
    allow_commands_again();
}

#[test]
fn a_folder_that_is_gone_is_not_reported_as_a_missing_program() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    allow_commands_again();

    let result = SystemRunner::new().run_in(
        std::path::Path::new("/definitely/not/here"),
        "sh",
        &["-c", "true"],
    );

    assert!(matches!(result, Err(RunError::Other(m)) if m.contains("not a folder")));
}

#[test]
fn finished_commands_leave_the_list_and_terminating_with_none_is_harmless() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    allow_commands_again();
    let out = SystemRunner::new().run("sh", &["-c", "echo hi"]).unwrap();

    assert!(out.success);
    assert_eq!(running_commands(), 0);
    terminate_running_commands();
}
