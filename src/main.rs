use std::path::PathBuf;

use clap::{Parser, Subcommand};
use piki_multiplex::session::client::ensure_daemon;
use piki_multiplex::session::daemon::{DaemonPaths, run};

#[derive(Parser)]
#[command(
    name = "piki-sessions",
    bin_name = "pks",
    version,
    about = "tmux-style terminal multiplexer on piki-multiplex"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Run the TUI (default).
    #[arg(long, hide = true)]
    tui: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the session daemon (foreground).
    Serve {
        /// Stay in the foreground instead of daemonizing.
        #[arg(long)]
        foreground: bool,
    },
    /// List sessions in the daemon.
    List,
    /// Attach to an existing session (placeholder).
    Attach { id: String },
    /// Shut down the session daemon.
    Shutdown {
        /// Force shutdown even with live sessions.
        #[arg(long)]
        force: bool,
    },
}

fn daemon_paths() -> DaemonPaths {
    let base = tui::data_dir();
    DaemonPaths {
        sessions_dir: base.join("sessions"),
        log_dir: base.join("logs"),
        lock_path: base.join("sessions").join("daemon.lock"),
        pid_path: base.join("sessions").join("daemon.pid"),
        socket_path: base.join("sessions").join("daemon.sock"),
        log_path: base.join("logs").join("session-daemon.log"),
    }
}

fn socket_path() -> PathBuf {
    daemon_paths().socket_path
}

mod tui;

fn main() {
    // Daemon must be handled before any tokio runtime / threads.
    let cli = Cli::parse();
    if let Some(Commands::Serve { foreground }) = cli.command {
        let paths = daemon_paths();
        if let Err(e) = run(&paths, foreground, None) {
            eprintln!("daemon error: {e}");
            std::process::exit(1);
        }
        return;
    }

    // TUI path (no subcommand): run synchronously.
    if cli.command.is_none() {
        if let Err(e) = tui::run() {
            eprintln!("tui error: {e}");
            std::process::exit(1);
        }
        return;
    }

    // Normal client path.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async_main(cli));
}

async fn async_main(cli: Cli) {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let sock = socket_path();
    let daemon = match ensure_daemon(&sock, tui::launch_daemon) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("could not reach daemon: {e}");
            std::process::exit(1);
        }
    };

    match cli.command {
        Some(Commands::List) => match daemon.list() {
            Ok(sessions) => {
                if sessions.is_empty() {
                    println!("no sessions");
                } else {
                    for s in sessions {
                        println!(
                            "{}  {}  ({}x{})  {}",
                            s.id,
                            if s.state.is_live() { "live" } else { "exited" },
                            s.rows,
                            s.cols,
                            s.command
                        );
                    }
                }
            }
            Err(e) => eprintln!("list failed: {e}"),
        },
        Some(Commands::Attach { id }) => {
            let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
            let cols = if cols < 4 { 80 } else { cols };
            let rows = if rows < 4 { 24 } else { rows };
            match daemon.attach(&id, rows, cols) {
                Ok(att) => {
                    if let Err(e) = tui::attach(att) {
                        eprintln!("attach failed: {e}");
                    }
                }
                Err(e) => eprintln!("attach failed: {e}"),
            }
        }
        Some(Commands::Shutdown { force }) => match daemon.shutdown(force) {
            Ok(()) => println!("daemon shutting down"),
            Err(e) => eprintln!("shutdown failed: {e}"),
        },
        _ => {
            println!("unknown command");
        }
    }
}
