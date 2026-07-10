//! Claude Code status widget — always-on-top desktop card fed by Claude Code hooks.
//!
//! Usage:
//!   claude-widget                   run the widget (default)
//!   claude-widget install           merge the widget's hooks + statusline into ~/.claude/settings.json
//!   claude-widget uninstall         remove them again
//!   claude-widget statusline        statusline emitter (Claude Code runs this, not you)
//!   claude-widget usage-api on|off  opt in/out of polling the account usage endpoint

mod app;
mod config;
mod debug_log;
mod emitter;
mod installer;
mod usage_api;
mod registry;
mod server;
mod sticky;
mod transcript;

/// Loopback port the hooks POST to and the widget listens on.
const PORT: u16 = 43110;

/// The port the *listener* binds. CW_PORT overrides it so a hermetic test instance can
/// run beside the real widget; `install`/`uninstall` always write the default port.
fn listen_port() -> u16 {
    std::env::var("CW_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(PORT)
}

fn main() -> eframe::Result<()> {
    match std::env::args().nth(1).as_deref() {
        // The statusline emitter: headless, must work with no display. Claude Code
        // invokes it on every statusline refresh with the payload on stdin.
        Some("statusline") => {
            emitter::run();
            return Ok(());
        }
        Some("install") => {
            match installer::install(PORT) {
                Ok((p, outcome)) => {
                    println!("Installed Claude-widget hooks -> {}", p.display());
                    match outcome {
                        installer::StatuslineOutcome::Installed => {
                            println!("Statusline emitter configured (feeds the usage gauge from terminal sessions).");
                        }
                        installer::StatuslineOutcome::KeptForeign(cmd) => {
                            eprintln!("WARNING: you already have a statusline ({cmd}); it was left untouched.");
                            eprintln!("The usage gauge stays empty unless your statusline also runs: claude-widget statusline");
                        }
                    }
                    println!("Restart your Claude Code session so the hooks load.");
                }
                Err(e) => {
                    eprintln!("claude-widget: install failed: {e}");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }
        Some("uninstall") => {
            match installer::uninstall(PORT) {
                Ok(p) => println!("Removed Claude-widget hooks -> {}", p.display()),
                Err(e) => {
                    eprintln!("claude-widget: uninstall failed: {e}");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }
        Some("usage-api") => {
            let path = config::config_path();
            match std::env::args().nth(2).as_deref() {
                Some("on") => match config::set_usage_api(&path, true) {
                    Ok(()) => {
                        println!("Usage-API polling ENABLED (persisted in {}).", path.display());
                        println!("A running widget picks this up within seconds. While enabled, it will:");
                        println!("  - read the stored OAuth access token from ~/.claude/.credentials.json");
                        println!("  - GET {} every {}s (backing off on 429)", usage_api::ENDPOINT, usage_api::POLL_SECS);
                        println!("  - write the 5h/7d usage snapshot the card displays");
                        println!("It never calls any token-refresh endpoint and never logs the token.");
                        println!("Disable anytime: claude-widget usage-api off");
                    }
                    Err(e) => {
                        eprintln!("claude-widget: could not persist opt-in: {e}");
                        std::process::exit(1);
                    }
                },
                Some("off") => match config::set_usage_api(&path, false) {
                    Ok(()) => {
                        println!("Usage-API polling disabled.");
                        println!("A running widget stops before its next poll — no restart needed.");
                    }
                    Err(e) => {
                        eprintln!("claude-widget: could not persist opt-out: {e}");
                        std::process::exit(1);
                    }
                },
                _ => {
                    eprintln!("usage: claude-widget usage-api on|off");
                    std::process::exit(2);
                }
            }
            return Ok(());
        }
        Some("--help") | Some("-h") => {
            println!("claude-widget [install|uninstall|statusline|usage-api on|off]  (no arg = run the widget)");
            return Ok(());
        }
        _ => {}
    }

    // X11-only: fail loudly on Wayland rather than misbehaving silently. Treat an
    // explicit wayland session type, or a Wayland socket without an X11 session, as Wayland.
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let is_wayland = session.eq_ignore_ascii_case("wayland")
        || (std::env::var_os("WAYLAND_DISPLAY").is_some() && !session.eq_ignore_ascii_case("x11"));
    if is_wayland {
        eprintln!(
            "claude-widget requires an X11 session (detected Wayland).\n\
             Log in choosing 'Ubuntu on Xorg', or run under X11."
        );
        std::process::exit(1);
    }

    // Single instance: bind the listener up front and keep it. If the port is already
    // taken, another widget is running. The same socket is handed to the listener
    // thread, so there is no free-then-rebind race.
    let port = listen_port();
    let listener = match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(_) => {
            eprintln!("claude-widget is already running (port {port} in use).");
            return Ok(());
        }
    };

    // The usage-API poller thread always runs but re-reads the persisted opt-in before
    // every request (issue #14) — so `usage-api on|off` takes effect on a running
    // widget without a restart, and no request is ever made while opted out.
    {
        let debug = std::env::var_os("CW_DEBUG").is_some();
        usage_api::spawn(move |line| {
            if debug {
                debug_log::append(&line);
            }
        });
    }

    let (tx, rx) = std::sync::mpsc::channel();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([270.0, 240.0])
            .with_min_inner_size([220.0, 140.0])
            .with_decorations(false)
            .with_window_level(egui::WindowLevel::AlwaysOnTop)
            .with_taskbar(false)
            .with_transparent(false)
            .with_resizable(true)
            .with_title("Claude Widget"),
        ..Default::default()
    };

    eframe::run_native(
        "Claude Widget",
        options,
        Box::new(move |cc| {
            server::spawn(listener, tx, cc.egui_ctx.clone());
            sticky::spawn_make_sticky();
            // Heartbeat: reliably wake the UI ~1s so the registry sync and the stall
            // backstop run even while idle and unfocused (request_repaint_after alone is
            // not honored by winit when the loop is otherwise asleep). Events still wake
            // it instantly via the listener's request_repaint.
            let tick_ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                tick_ctx.request_repaint();
            });
            Ok(Box::new(app::WidgetApp::new(rx)))
        }),
    )
}
