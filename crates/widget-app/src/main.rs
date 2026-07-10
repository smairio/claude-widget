//! Claude Code status widget — always-on-top desktop card fed by Claude Code hooks.
//!
//! Usage:
//!   claude-widget             run the widget (default)
//!   claude-widget install     merge the widget's hooks + statusline into ~/.claude/settings.json
//!   claude-widget uninstall   remove them again
//!   claude-widget statusline  statusline emitter (Claude Code runs this, not you)

mod app;
mod emitter;
mod installer;
mod registry;
mod server;
mod sticky;
mod transcript;

/// Loopback port the hooks POST to and the widget listens on.
const PORT: u16 = 43110;

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
        Some("--help") | Some("-h") => {
            println!("claude-widget [install|uninstall|statusline]  (no arg = run the widget)");
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
    let listener = match std::net::TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(l) => l,
        Err(_) => {
            eprintln!("claude-widget is already running (port {PORT} in use).");
            return Ok(());
        }
    };

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
