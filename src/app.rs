use anyhow::Result;
use std::io::Stdout;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event as CtEvent, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::events::{ChannelReporter, Command, Event, RecordingReporter, Reporter};
use crate::orchestrator;
use crate::tui::{self, AppState};

type Term = Terminal<CrosstermBackend<Stdout>>;

fn setup_terminal() -> Result<Term> {
    enable_raw_mode()?;
    let mut out = std::io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let term = Terminal::new(CrosstermBackend::new(out))?;
    Ok(term)
}

fn restore_terminal(term: &mut Term) -> Result<()> {
    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;
    Ok(())
}

/// While alive, the process's stderr (fd 2) is redirected to a log file so the
/// orchestrator's `eprintln!` diagnostics don't scroll over the alt-screen TUI.
/// Dropping it restores the original stderr. Unix-only; a no-op elsewhere.
#[cfg(unix)]
struct StderrRedirect {
    saved: std::os::unix::io::RawFd,
}

#[cfg(unix)]
impl StderrRedirect {
    fn to_log(log_dir: &Path) -> Option<Self> {
        use std::os::unix::io::AsRawFd;
        let _ = std::fs::create_dir_all(log_dir);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("run.log"))
            .ok()?;
        let stderr_fd = std::io::stderr().as_raw_fd();
        // SAFETY: dup/dup2/close are plain libc calls operating on valid fds; the
        // file stays open for the duration of the dup2 call. nix's safe wrappers for
        // these are gated behind the `fs` feature, which this crate does not enable.
        let saved = unsafe { nix::libc::dup(stderr_fd) };
        if saved < 0 {
            return None;
        }
        if unsafe { nix::libc::dup2(file.as_raw_fd(), stderr_fd) } < 0 {
            unsafe { nix::libc::close(saved) };
            return None;
        }
        Some(StderrRedirect { saved })
    }
}

#[cfg(unix)]
impl Drop for StderrRedirect {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        let stderr_fd = std::io::stderr().as_raw_fd();
        // SAFETY: see `to_log`; `self.saved` is a valid fd we own and close here.
        unsafe {
            nix::libc::dup2(self.saved, stderr_fd);
            nix::libc::close(self.saved);
        }
    }
}

/// On SIGINT/SIGTERM while the TUI path is active, raw mode may be on and the alt
/// screen active. Restore the terminal best-effort, kill in-flight agents, and exit
/// so nothing is orphaned. (While the TUI event loop runs, raw mode swallows Ctrl-C
/// into a key event; this covers the startup/shutdown windows where raw mode is off.)
#[cfg(unix)]
fn install_tui_signal_handler() {
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let (Ok(mut term), Ok(mut int)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) else {
            return;
        };
        let code = tokio::select! {
            _ = term.recv() => 143,
            _ = int.recv() => 130,
        };
        let _ = disable_raw_mode();
        let _ = execute!(
            std::io::stdout(),
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        crate::spawn::kill_all_agents();
        std::process::exit(code);
    });
}

/// Restore the terminal on panic. Without this, any panic while the TUI is up
/// (raw mode + alternate screen) unwinds past `restore_terminal` and dumps the
/// user into a broken shell — no echo, no line discipline — until `reset`.
/// Chained in front of the default hook so the panic message still prints,
/// now onto a usable screen.
fn install_panic_terminal_restore() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            std::io::stdout(),
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        default_hook(info);
    }));
}

pub async fn run_tui(cfg: Config, ws: PathBuf, goal: String) -> Result<i32> {
    install_panic_terminal_restore();
    #[cfg(unix)]
    install_tui_signal_handler();

    let (etx, mut erx) = mpsc::unbounded_channel::<Event>();
    let (ctx, mut crx) = mpsc::unbounded_channel::<Command>();

    let reporter: Arc<dyn Reporter> = Arc::new(RecordingReporter::new(
        ws.clone(),
        Arc::new(ChannelReporter::new(etx)),
    ));
    let cfg_o = cfg.clone();
    let ws_o = ws.clone();
    let orch = tokio::spawn(async move {
        orchestrator::run_interactive(&cfg_o, &ws_o, reporter, &mut crx).await
    });

    let mut term = setup_terminal()?;
    #[cfg(unix)]
    let _stderr_guard = StderrRedirect::to_log(&ws.join(".agentloop/logs"));
    let mut state = AppState::new(goal);

    // Track whether the orchestrator has disconnected its event sender.
    let mut orch_done = false;

    // Event loop. Returns Ok(()) on normal exit.
    let loop_res: Result<()> = loop {
        // Drain orchestrator events.
        loop {
            match erx.try_recv() {
                Ok(ev) => state.apply(ev),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    orch_done = true;
                    break;
                }
            }
        }

        if term.draw(|f| tui::render(f, &state)).is_err() {
            break Ok(());
        }

        // Poll for a key with a short timeout (keeps the tick ~80 ms).
        if event::poll(Duration::from_millis(80)).unwrap_or(false) {
            if let Ok(CtEvent::Key(k)) = event::read() {
                // Raw mode swallows the SIGINT that Ctrl-C would otherwise raise, so
                // handle Ctrl-C / Ctrl-D here as an explicit quit.
                if k.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('d'))
                {
                    break Ok(());
                }
                if let Some(cmd) = state.on_key(k) {
                    let quit = matches!(cmd, Command::Quit);
                    let _ = ctx.send(cmd);
                    if quit {
                        break Ok(());
                    }
                }
            }
        }

        // Exit when the orchestrator has finished (event channel closed & empty).
        if orch_done || (erx.is_closed() && erx.is_empty()) {
            break Ok(());
        }
    };

    let _ = restore_terminal(&mut term);
    loop_res?;

    // Tell the orchestrator to stop, and kill any in-flight agents so quitting the TUI
    // never leaves orphaned claude/codex running. Killing the agents also makes the
    // orchestrator's current iteration return promptly.
    let _ = ctx.send(Command::Quit);
    crate::spawn::kill_all_agents();
    let rc = orch.await.unwrap_or(Ok(1)).unwrap_or(1);
    Ok(rc)
}
