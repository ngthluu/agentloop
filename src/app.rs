use anyhow::Result;
use std::io::Stdout;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event as CtEvent};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::events::{ChannelReporter, Command, Event, Reporter};
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

pub async fn run_tui(cfg: Config, ws: PathBuf, goal: String) -> Result<i32> {
    let (etx, mut erx) = mpsc::unbounded_channel::<Event>();
    let (ctx, mut crx) = mpsc::unbounded_channel::<Command>();

    let reporter: Arc<dyn Reporter> = Arc::new(ChannelReporter::new(etx));
    let cfg_o = cfg.clone();
    let ws_o = ws.clone();
    let orch = tokio::spawn(async move {
        orchestrator::run_interactive(&cfg_o, &ws_o, reporter, &mut crx).await
    });

    let mut term = setup_terminal()?;
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

    let _ = ctx.send(Command::Quit);
    let rc = orch.await.unwrap_or(Ok(1)).unwrap_or(1);
    Ok(rc)
}
