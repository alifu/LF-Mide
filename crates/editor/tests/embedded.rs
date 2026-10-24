//! The embedded session against a real Neovim (`nvim` on PATH; skipped
//! otherwise).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use lf_mide_core::screen::{CursorShape, Screen};
use lf_mide_editor::embedded::{Launch, Session, SessionEvent};
use lf_mide_editor::lsp::LspServers;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

const PATIENCE: Duration = Duration::from_secs(10);

fn nvim_available() -> bool {
    let found = Command::new("nvim")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !found {
        eprintln!("SKIPPED: nvim is not installed, so the embedded editor is not tested");
    }
    found
}

struct Running {
    session: Session,
    events: UnboundedReceiver<SessionEvent>,
    latest: Option<Arc<Screen>>,
    dir: tempfile::TempDir,
}

impl Running {
    fn socket(&self) -> PathBuf {
        self.dir.path().join("nvim.sock")
    }

    /// Waits for an event matching `wanted`, keeping the latest screen.
    async fn until(&mut self, what: &str, wanted: impl Fn(&SessionEvent) -> bool) -> SessionEvent {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            let event = tokio::time::timeout_at(deadline, self.events.recv())
                .await
                .unwrap_or_else(|_| {
                    panic!("timed out waiting for {what}; screen: {:#?}", self.text())
                })
                .expect("the session stopped reporting");
            if let SessionEvent::Screen(screen) = &event {
                self.latest = Some(Arc::clone(screen));
            }
            if wanted(&event) {
                return event;
            }
        }
    }

    async fn until_screen(&mut self, what: &str, wanted: impl Fn(&Screen) -> bool) -> Arc<Screen> {
        if let Some(screen) = self.latest.clone().filter(|s| wanted(s)) {
            return screen;
        }
        match self
            .until(what, |e| matches!(e, SessionEvent::Screen(s) if wanted(s)))
            .await
        {
            SessionEvent::Screen(screen) => screen,
            _ => unreachable!(),
        }
    }

    fn text(&self) -> Vec<String> {
        self.latest
            .as_ref()
            .map(|s| (0..s.height).map(|r| s.row_text(r)).collect())
            .unwrap_or_default()
    }
}

async fn start_with(width: u16, height: u16, lsp: Option<LspServers>) -> Running {
    let dir = tempfile::TempDir::new().unwrap();
    let (tx, events) = unbounded_channel();
    let launch = Launch {
        program: "nvim".into(),
        args: vec!["--clean".into()],
        cwd: dir.path().to_path_buf(),
        socket: dir.path().join("nvim.sock"),
        width,
        height,
        lsp,
    };
    let session = Session::start(launch, move |event| {
        let _ = tx.send(event);
    })
    .await
    .unwrap();
    let mut running = Running {
        session,
        events,
        latest: None,
        dir,
    };
    running
        .until("ready", |e| matches!(e, SessionEvent::Ready { .. }))
        .await;
    running
}

async fn start(width: u16, height: u16) -> Running {
    start_with(width, height, None).await
}

fn starts_with(screen: &Screen, row: u16, text: &str) -> bool {
    screen.row_text(row).starts_with(text)
}

#[tokio::test]
async fn typed_keys_show_on_the_screen_with_the_mode_cursor() {
    if !nvim_available() {
        return;
    }
    let mut nvim = start(40, 10).await;

    nvim.session.input("ihello <lt>world>".into()).unwrap();
    let typing = nvim
        .until_screen("typed text", |s| starts_with(s, 0, "hello <world>"))
        .await;
    nvim.session.input("<Esc>".into()).unwrap();
    let normal = nvim
        .until_screen("normal mode", |s| s.cursor_shape() == CursorShape::Block)
        .await;

    assert_eq!((typing.width, typing.height), (40, 10));
    assert_eq!(typing.cursor_shape(), CursorShape::Vertical);
    assert_eq!(normal.cursor, (0, 12), "on the last character after Esc");
}

#[tokio::test]
async fn a_resize_changes_the_grid() {
    if !nvim_available() {
        return;
    }
    let mut nvim = start(40, 10).await;

    nvim.session.resize(30, 5).unwrap();
    let screen = nvim
        .until_screen("resized grid", |s| (s.width, s.height) == (30, 5))
        .await;

    assert_eq!(screen.row_text(0).chars().count(), 30);
}

#[tokio::test]
async fn a_paste_inserts_every_line_once() {
    if !nvim_available() {
        return;
    }
    let mut nvim = start(40, 10).await;

    nvim.session.paste("one\r\ntwo\rthree".into()).unwrap();
    let screen = nvim
        .until_screen("pasted lines", |s| starts_with(s, 2, "three"))
        .await;

    assert!(starts_with(&screen, 0, "one"));
    assert!(starts_with(&screen, 1, "two"));
}

#[tokio::test]
async fn files_open_through_the_socket_as_with_a_split_pane() {
    if !nvim_available() {
        return;
    }
    let mut nvim = start(40, 10).await;
    let file = nvim.dir.path().join("Main.kt");
    std::fs::write(&file, "fun main() = println(\"hi\")\n").unwrap();

    let name = lf_mide_editor::open_file(&nvim.socket(), &file, Some(1), 0, PATIENCE)
        .await
        .unwrap();
    nvim.until_screen("file contents", |s| starts_with(s, 0, "fun main()"))
        .await;

    assert!(name.ends_with("Main.kt"), "{name}");
}

#[tokio::test]
async fn quitting_without_changes_ends_neovim() {
    if !nvim_available() {
        return;
    }
    let mut nvim = start(40, 10).await;

    nvim.session.confirm_quit().unwrap();

    nvim.until("exit", |e| *e == SessionEvent::Exited).await;
}

#[tokio::test]
async fn unsaved_changes_are_asked_about_and_cancel_keeps_neovim() {
    if !nvim_available() {
        return;
    }
    let mut nvim = start(60, 10).await;
    nvim.session.input("ichanged<Esc>".into()).unwrap();
    nvim.until_screen("edit", |s| starts_with(s, 0, "changed"))
        .await;

    nvim.session.confirm_quit().unwrap();
    nvim.until_screen("prompt", |s| {
        (0..s.height).any(|r| s.row_text(r).contains("Save changes"))
    })
    .await;
    nvim.session.input("c".into()).unwrap();
    nvim.until("cancel", |e| {
        *e == SessionEvent::QuitCancelled { reason: None }
    })
    .await;
    // Clear the old prompt so the next one is known to be new.
    nvim.session.input("<C-l>".into()).unwrap();
    nvim.until_screen("prompt gone", |s| {
        !(0..s.height).any(|r| s.row_text(r).contains("Save changes"))
    })
    .await;
    assert!(
        starts_with(nvim.latest.as_ref().unwrap(), 0, "changed"),
        "still open"
    );

    nvim.session.confirm_quit().unwrap();
    nvim.until_screen("prompt again", |s| {
        (0..s.height).any(|r| s.row_text(r).contains("Save changes"))
    })
    .await;
    nvim.session.input("n".into()).unwrap();
    nvim.until("exit", |e| *e == SessionEvent::Exited).await;
}

#[tokio::test]
async fn terminate_ends_the_process_and_shutdown_can_wait_for_it() {
    if !nvim_available() {
        return;
    }
    let mut nvim = start(40, 10).await;

    nvim.session.terminate();

    assert!(nvim.session.wait_exit(PATIENCE).await, "ended in time");
    nvim.until("exit", |e| *e == SessionEvent::Exited).await;
}

#[tokio::test]
async fn a_quit_neovim_refuses_is_reported_with_its_reason() {
    if !nvim_available() {
        return;
    }
    let mut nvim = start(60, 10).await;
    let folder = nvim.dir.path().join("gone");
    std::fs::create_dir(&folder).unwrap();
    let file = folder.join("x.txt");
    std::fs::write(&file, "x\n").unwrap();
    lf_mide_editor::open_file(&nvim.socket(), &file, None, 0, PATIENCE)
        .await
        .unwrap();
    nvim.session.input("Achanged<Esc>".into()).unwrap();
    nvim.until_screen("edit", |s| starts_with(s, 0, "xchanged"))
        .await;
    // Saving will fail: the file's folder is gone.
    std::fs::remove_dir_all(&folder).unwrap();

    nvim.session.confirm_quit().unwrap();
    nvim.until_screen("prompt", |s| {
        (0..s.height).any(|r| s.row_text(r).contains("Save changes"))
    })
    .await;
    nvim.session.input("y".into()).unwrap();
    let event = nvim
        .until("refusal", |e| {
            matches!(e, SessionEvent::QuitCancelled { .. })
        })
        .await;

    let SessionEvent::QuitCancelled { reason } = event else {
        unreachable!()
    };
    assert!(
        reason.as_deref().is_some_and(|r| r.starts_with("E212")),
        "{reason:?}"
    );
    assert!(!nvim.session.wait_exit(Duration::from_millis(300)).await);
}

#[tokio::test]
async fn a_missing_program_is_an_error() {
    let dir = tempfile::TempDir::new().unwrap();
    let launch = Launch {
        program: "/nonexistent/nvim".into(),
        args: Vec::new(),
        cwd: dir.path().to_path_buf(),
        socket: dir.path().join("s"),
        width: 10,
        height: 5,
        lsp: None,
    };

    let result = Session::start(launch, |_| {}).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn language_servers_are_registered_with_neovim() {
    if !nvim_available() {
        return;
    }
    let servers = LspServers {
        kotlin: Some(PathBuf::from("/usr/bin/true")),
        swift: None,
    };
    let nvim = start_with(40, 10, Some(servers)).await;

    let enabled = lua(&nvim.socket(), "return { vim.lsp.is_enabled('lf_mide_kotlin'), vim.lsp.is_enabled('lf_mide_swift'), vim.o.completeopt }").await;

    assert_eq!(
        enabled,
        nvim_rs::Value::Array(vec![
            true.into(),
            false.into(),
            "menu,menuone,noselect,popup".into()
        ])
    );
}

async fn lua(socket: &Path, code: &str) -> nvim_rs::Value {
    let (nvim, _io) = nvim_rs::create::tokio::new_path(socket, nvim_rs::rpc::handler::Dummy::new())
        .await
        .unwrap();
    nvim.exec_lua(code, vec![]).await.unwrap()
}

#[tokio::test]
async fn resizing_and_pasting_while_neovim_asks_do_not_block_the_answer() {
    if !nvim_available() {
        return;
    }
    let mut nvim = start(60, 10).await;
    nvim.session.input("ichanged<Esc>".into()).unwrap();
    nvim.until_screen("edit", |s| starts_with(s, 0, "changed"))
        .await;

    nvim.session.confirm_quit().unwrap();
    nvim.until_screen("prompt", |s| {
        (0..s.height).any(|r| s.row_text(r).contains("Save changes"))
    })
    .await;
    nvim.session.resize(50, 9).unwrap();
    nvim.session.paste("x".into()).unwrap();
    nvim.session.input("c".into()).unwrap();

    nvim.until("cancel", |e| {
        matches!(e, SessionEvent::QuitCancelled { reason: None })
    })
    .await;
}
