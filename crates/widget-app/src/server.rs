//! Loopback HTTP listener that receives hook POSTs and feeds parsed events to the UI.

use std::io::Read;
use std::net::TcpListener;
use std::sync::mpsc::Sender;

use widget_core::{parse_hook, ParsedEvent};

/// Largest hook body we will read (hook payloads are a few KB; this just caps abuse).
const MAX_BODY: u64 = 64 * 1024;

/// Spawn the listener thread on an already-bound listener (bound once in `main` so the
/// single-instance check and the listening socket are the same object — no re-bind race).
/// Each POST body is a hook payload; valid ones are parsed, sent to the UI, and trigger a
/// repaint. Malformed bodies are dropped.
pub fn spawn(listener: TcpListener, tx: Sender<ParsedEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let server = match tiny_http::Server::from_listener(listener, None) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("claude-widget: could not serve on the bound socket: {e}");
                return;
            }
        };
        for mut request in server.incoming_requests() {
            let mut body = String::new();
            let _ = request.as_reader().take(MAX_BODY).read_to_string(&mut body);
            if let Ok(ev) = parse_hook(&body) {
                let _ = tx.send(ev);
                ctx.request_repaint();
            }
            let _ = request.respond(tiny_http::Response::from_string("ok"));
        }
    });
}
