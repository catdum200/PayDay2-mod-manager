#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod app;
mod config;
mod game;
mod installer;
mod model;
mod modpack;
mod nsfw;
mod theme;
mod worker;

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;
use worker::{Msg, Worker};

/// Local port used to make the app single-instance and to forward nxm:// links.
const IPC_ADDR: &str = "127.0.0.1:47391";
/// Handshake token so we never mistake another program on that port for ourselves.
const HELLO: &str = "PD2MM-1";

fn main() -> eframe::Result<()> {
    let arg = std::env::args().nth(1);
    let nxm = arg.clone().filter(|a| a.to_lowercase().starts_with("nxm://"));
    // A double-clicked .pd2pack / modpack .zip (file association set up by the installer).
    let open_file = arg.filter(|a| !a.to_lowercase().starts_with("nxm://")).map(std::path::PathBuf::from).filter(|p| p.is_file());

    // If another instance is running, hand it our nxm link (or just focus it) and exit.
    let listener = match TcpListener::bind(IPC_ADDR) {
        Ok(l) => Some(l),
        Err(_) => {
            // Only hand off if the port really belongs to another copy of this app;
            // otherwise some unrelated program is using it and we just start normally.
            if let Ok(mut s) = TcpStream::connect_timeout(&IPC_ADDR.parse().unwrap(), Duration::from_secs(2)) {
                let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                let line = match (&nxm, &open_file) {
                    (Some(link), _) => link.clone(),
                    (None, Some(p)) => format!("OPEN {}", p.display()),
                    _ => "FOCUS".into(),
                };
                if writeln!(s, "{HELLO} {line}").is_ok() {
                    let mut reply = String::new();
                    if BufReader::new(&s).read_line(&mut reply).is_ok() && reply.trim() == HELLO {
                        return Ok(());
                    }
                }
            }
            None
        }
    };

    let (tx, rx) = std::sync::mpsc::channel();
    let worker = Worker::new(tx);

    if let Some(listener) = listener {
        let w = worker.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut line = String::new();
                let ok = match stream.try_clone() {
                    Ok(s) => BufReader::new(s).read_line(&mut line).is_ok(),
                    Err(_) => false,
                };
                let Some(msg) = line.trim().strip_prefix(HELLO).map(str::trim) else { continue };
                if !ok {
                    continue;
                }
                let _ = writeln!(stream, "{HELLO}");
                if msg.to_lowercase().starts_with("nxm://") {
                    w.send(Msg::Nxm(msg.to_string()));
                } else if let Some(path) = msg.strip_prefix("OPEN ") {
                    w.send(Msg::OpenFile(path.into()));
                }
                w.send(Msg::Focus);
            }
        });
    }

    let icon = make_icon();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("PD2 Mod Manager")
            .with_inner_size([1320.0, 820.0])
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true)
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native(
        "PD2 Mod Manager",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, worker, rx, nxm, open_file)))),
    )
}

/// Simple generated window icon (rounded blue square with a white diamond).
fn make_icon() -> eframe::egui::IconData {
    let n = 64u32;
    let mut rgba = Vec::with_capacity((n * n * 4) as usize);
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 - 31.5, y as f32 - 31.5);
            let r = 12.0;
            let qx = (fx.abs() - (32.0 - r)).max(0.0);
            let qy = (fy.abs() - (32.0 - r)).max(0.0);
            let inside = (qx * qx + qy * qy).sqrt() <= r;
            let diamond = fx.abs() + fy.abs() <= 18.0 && fx.abs() + fy.abs() >= 10.0;
            let px = if !inside {
                [0, 0, 0, 0]
            } else if diamond {
                [255, 255, 255, 255]
            } else {
                [0x4F, 0x9C, 0xF9, 255]
            };
            rgba.extend_from_slice(&px);
        }
    }
    eframe::egui::IconData { rgba, width: n, height: n }
}
