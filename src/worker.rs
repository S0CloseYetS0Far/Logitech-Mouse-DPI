//! Background thread that owns all HID I/O, so slow probes (receiver slots,
//! sleeping wireless mice) never block the window.

use crate::access;
use crate::hidpp::{self, DpiInfo, Mouse};
use eframe::egui;
use hidapi::HidApi;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_secs(2);
/// How often to re-probe when nodes exist but no mouse answered.
const RETRY: Duration = Duration::from_secs(10);

pub enum Cmd {
    Select(usize),
    Apply(u16),
    /// Install the udev rule (asks for the password once).
    FixAccess,
}

/// What the window shows; replaced wholesale on every change.
#[derive(Clone, Default, PartialEq)]
pub struct Snapshot {
    pub mice: Vec<String>,
    pub selected: usize,
    pub info: Option<DpiInfo>,
    pub status: String,
    pub error: Option<String>,
    /// Logitech devices exist but the udev rule is missing.
    pub denied: bool,
}

struct Worker {
    api: HidApi,
    nodes: Vec<hidpp::Node>,
    mice: Vec<Mouse>,
    selected: usize,
    info: Option<DpiInfo>,
    status: String,
    error: Option<String>,
    denied: bool,
    last_scan: Instant,
}

pub fn run(rx: Receiver<Cmd>, shared: Arc<Mutex<Snapshot>>, ctx: egui::Context) {
    let api = match HidApi::new() {
        Ok(api) => api,
        Err(e) => {
            let mut s = shared.lock().unwrap();
            s.status = "HID unavailable".into();
            s.error = Some(format!("Cannot initialise HID: {e}"));
            ctx.request_repaint();
            return;
        }
    };
    let mut w = Worker {
        api,
        nodes: Vec::new(),
        mice: Vec::new(),
        selected: 0,
        info: None,
        status: String::new(),
        error: None,
        denied: false,
        last_scan: Instant::now(),
    };
    w.rescan();
    loop {
        w.publish(&shared, &ctx);
        match rx.recv_timeout(TICK) {
            Ok(Cmd::Select(i)) if i < w.mice.len() => {
                w.selected = i;
                w.load_info();
            }
            Ok(Cmd::Select(_)) => {}
            Ok(Cmd::Apply(dpi)) => w.apply(dpi),
            Ok(Cmd::FixAccess) => {
                w.status = "Waiting for your password…".into();
                w.error = None;
                w.publish(&shared, &ctx);
                if let Err(e) = access::install_rule() {
                    w.status = "Permission needed to talk to your mouse".into();
                    w.error = Some(e);
                    continue;
                }
                w.rescan();
            }
            Err(RecvTimeoutError::Timeout) => w.tick(),
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

impl Worker {
    fn tick(&mut self) {
        let nodes = hidpp::candidate_nodes(&mut self.api);
        if nodes != self.nodes || (self.info.is_none() && self.last_scan.elapsed() >= RETRY) {
            self.rescan();
            return;
        }
        // Picks up DPI changes made with the mouse's own DPI buttons.
        let Some(mouse) = self.mice.get(self.selected) else { return };
        match mouse.read_current() {
            Ok((cur, _)) => {
                if let Some(info) = &mut self.info {
                    info.current = cur;
                }
            }
            Err(e) => self.lost(e.to_string()),
        }
    }

    fn rescan(&mut self) {
        self.last_scan = Instant::now();
        let keep = self.mice.get(self.selected).map(Mouse::key);
        self.nodes = hidpp::candidate_nodes(&mut self.api);
        let (mice, errors) = hidpp::discover(&self.api, &self.nodes);
        self.mice = mice;
        self.selected = keep
            .and_then(|k| self.mice.iter().position(|m| m.key() == k))
            .unwrap_or(0);
        self.error = errors.into_iter().next();
        self.denied = access::denied(&self.nodes);
        if self.mice.is_empty() {
            self.info = None;
            self.status = if self.denied {
                self.error = None;
                "Permission needed to talk to your mouse".into()
            } else if self.nodes.is_empty() {
                "No Logitech mouse found — plug one in".into()
            } else {
                "No Logitech mouse with adjustable DPI found".into()
            };
        } else {
            self.load_info();
        }
    }

    fn load_info(&mut self) {
        let mouse = &self.mice[self.selected];
        match mouse.read_info() {
            Ok(info) => {
                self.status = format!("{} connected", mouse.name);
                self.error = None;
                self.info = Some(info);
            }
            Err(e) => self.lost(e.to_string()),
        }
    }

    fn apply(&mut self, dpi: u16) {
        let Some(mouse) = self.mice.get(self.selected) else { return };
        match mouse.set_dpi(dpi).and_then(|_| mouse.read_current()) {
            Ok((cur, _)) => {
                if let Some(info) = &mut self.info {
                    info.current = cur;
                }
                if cur == dpi {
                    self.status = format!("{} — set to {cur} DPI", mouse.name);
                    self.error = None;
                } else {
                    self.error = Some(format!(
                        "Mouse stayed at {cur} DPI; its onboard profile may be in control"
                    ));
                }
            }
            Err(e) => self.lost(e.to_string()),
        }
    }

    /// The selected mouse stopped answering: drop its info and re-probe next tick.
    fn lost(&mut self, e: String) {
        self.info = None;
        self.status = match self.mice.get(self.selected) {
            Some(m) => format!("{} not responding", m.name),
            None => "Mouse not connected".into(),
        };
        self.error = Some(e);
        self.nodes.clear();
    }

    fn publish(&self, shared: &Mutex<Snapshot>, ctx: &egui::Context) {
        let snap = Snapshot {
            mice: self.mice.iter().map(|m| m.name.clone()).collect(),
            selected: self.selected,
            info: self.info.clone(),
            status: self.status.clone(),
            error: self.error.clone(),
            denied: self.denied,
        };
        let mut s = shared.lock().unwrap();
        if *s != snap {
            *s = snap;
            ctx.request_repaint();
        }
    }
}
