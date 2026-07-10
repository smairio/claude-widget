//! Make our own window sticky (visible on all workspaces) and always-above on X11.
//!
//! winit/eframe give us always-on-top and skip-taskbar directly, but not "sticky"
//! (all workspaces). We set the EWMH `_NET_WM_STATE_STICKY` + `_NET_WM_STATE_ABOVE`
//! states on our window by client message — the standard, WM-agnostic way (Mutter on
//! GNOME/X11 honors both). We find our window by matching `_NET_WM_PID` (winit sets it).

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, Window};

/// Retry for ~5s in the background until our window is mapped and found, then set the states.
pub fn spawn_make_sticky() {
    std::thread::spawn(|| {
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            match try_once() {
                Ok(true) => return,
                _ => continue,
            }
        }
        eprintln!("claude-widget: could not set sticky/always-on-top state (window not found)");
    });
}

fn intern(conn: &impl Connection, name: &str) -> Option<u32> {
    conn.intern_atom(false, name.as_bytes())
        .ok()?
        .reply()
        .ok()
        .map(|r| r.atom)
}

fn try_once() -> Result<bool, Box<dyn std::error::Error>> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen_num].root;

    let net_client_list = intern(&conn, "_NET_CLIENT_LIST").ok_or("no _NET_CLIENT_LIST")?;
    let net_wm_pid = intern(&conn, "_NET_WM_PID").ok_or("no _NET_WM_PID")?;
    let net_wm_state = intern(&conn, "_NET_WM_STATE").ok_or("no _NET_WM_STATE")?;
    let sticky = intern(&conn, "_NET_WM_STATE_STICKY").ok_or("no _STICKY")?;
    let above = intern(&conn, "_NET_WM_STATE_ABOVE").ok_or("no _ABOVE")?;

    let our_pid = std::process::id();

    let list = conn
        .get_property(false, root, net_client_list, AtomEnum::WINDOW, 0, u32::MAX)?
        .reply()?;
    let windows: Vec<Window> = match list.value32() {
        Some(v) => v.collect(),
        None => return Ok(false),
    };

    for w in windows {
        let pid = conn
            .get_property(false, w, net_wm_pid, AtomEnum::CARDINAL, 0, 1)?
            .reply()?;
        // Read the first CARDINAL and drop the borrowing iterator in the same statement.
        let first_pid = pid.value32().and_then(|mut it| it.next());
        if first_pid == Some(our_pid) {
            // _NET_WM_STATE client message: [action=ADD(1), atom1, atom2, source=app(1), 0]
            let data = [1u32, sticky, above, 1, 0];
            let ev = ClientMessageEvent::new(32, w, net_wm_state, data);
            conn.send_event(
                false,
                root,
                EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
                ev,
            )?;
            conn.flush()?;
            return Ok(true);
        }
    }
    Ok(false)
}
