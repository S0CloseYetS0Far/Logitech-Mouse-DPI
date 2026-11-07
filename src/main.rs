mod access;
mod hidpp;
mod worker;

use eframe::egui::{self, Color32, RichText, Stroke};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use worker::{Cmd, Snapshot};

const BG: Color32 = Color32::BLACK;
const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe6, 0xe6);
const DIM: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x8a);
const ACCENT: Color32 = Color32::from_rgb(0x4f, 0x8c, 0xc9);
const ERR: Color32 = Color32::from_rgb(0xd0, 0x6a, 0x6a);

struct App {
    tx: Sender<Cmd>,
    shared: Arc<Mutex<Snapshot>>,
    target: u16,
    /// (selected mouse, its DPI) last time we looked, to follow DPI-button presses.
    seen: Option<(usize, u16)>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        apply_style(&cc.egui_ctx);
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(Snapshot {
            status: "Searching for Logitech mice…".into(),
            ..Default::default()
        }));
        let (s, ctx) = (shared.clone(), cc.egui_ctx.clone());
        std::thread::spawn(move || worker::run(rx, s, ctx));
        App { tx, shared, target: 0, seen: None }
    }
}

/// Nearest supported value.
fn snap(values: &[u16], v: u16) -> u16 {
    *values.iter().min_by_key(|&&d| d.abs_diff(v)).unwrap_or(&v)
}

fn describe_range(values: &[u16]) -> String {
    let (min, max) = (values[0], values[values.len() - 1]);
    let step = values.get(1).map(|v| v - min);
    let uniform = step.is_some_and(|s| values.windows(2).all(|w| w[1] - w[0] == s));
    match step {
        Some(s) if uniform => format!("{min}–{max} DPI, step {s}"),
        _ => format!("{min}–{max} DPI, {} values", values.len()),
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let snap_state = self.shared.lock().unwrap().clone();
        if let Some(info) = &snap_state.info {
            let now = (snap_state.selected, info.current);
            let follow = match self.seen {
                Some((sel, cur)) => sel != now.0 || self.target == cur,
                None => true,
            };
            if follow {
                self.target = info.current;
            }
            self.seen = Some(now);
        } else {
            self.seen = None;
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BG).inner_margin(20.0))
            .show(ui, |ui| {
                let info = snap_state.info.as_ref();

                // Mouse name, or a picker when several are connected.
                ui.vertical_centered(|ui| match snap_state.mice.len() {
                    0 => {}
                    1 => {
                        ui.label(RichText::new(&snap_state.mice[0]).size(13.0).color(DIM));
                    }
                    _ => {
                        let mut sel = snap_state.selected;
                        egui::ComboBox::from_id_salt("mouse")
                            .selected_text(&snap_state.mice[sel])
                            .width(ui.available_width() * 0.8)
                            .show_ui(ui, |ui| {
                                for (i, name) in snap_state.mice.iter().enumerate() {
                                    ui.selectable_value(&mut sel, i, format!("{}. {name}", i + 1));
                                }
                            });
                        if sel != snap_state.selected {
                            let _ = self.tx.send(Cmd::Select(sel));
                        }
                    }
                });

                ui.vertical_centered(|ui| {
                    let big = info.map_or("—".into(), |i| i.current.to_string());
                    ui.label(RichText::new(big).size(64.0).strong().color(TEXT));
                    ui.label(RichText::new("current DPI").size(13.0).color(DIM));
                });
                ui.add_space(20.0);

                if snap_state.denied && info.is_none() {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("Linux needs your permission once before\nthis app can talk to Logitech mice.")
                                .size(13.0)
                                .color(DIM),
                        );
                        ui.add_space(10.0);
                        let btn = egui::Button::new(RichText::new("Allow access").size(16.0).color(Color32::WHITE))
                            .fill(ACCENT)
                            .min_size(egui::vec2(160.0, 34.0));
                        if ui.add(btn).clicked() {
                            let _ = self.tx.send(Cmd::FixAccess);
                        }
                    });
                } else {
                    ui.add_enabled_ui(info.is_some(), |ui| {
                        let values: &[u16] = info.map_or(&[0, 1], |i| &i.values);
                        let (min, max) = (values[0], values[values.len() - 1]);
                        ui.spacing_mut().slider_width = ui.available_width() - 90.0;
                        ui.horizontal(|ui| {
                            // The slider moves through the supported values by index,
                            // so uneven DPI lists still land on valid values.
                            let mut idx = values.iter().position(|&v| v >= self.target).unwrap_or(values.len() - 1);
                            let slider = egui::Slider::new(&mut idx, 0..=values.len() - 1)
                                .show_value(false)
                                .trailing_fill(true);
                            if ui.add(slider).changed() {
                                self.target = values[idx];
                            }
                            let step = values.get(1).map_or(1, |v| v - min).max(1);
                            let drag = ui.add(
                                egui::DragValue::new(&mut self.target)
                                    .range(min..=max)
                                    .speed(step as f64),
                            );
                            if drag.lost_focus() || drag.drag_stopped() {
                                self.target = snap(values, self.target);
                            }
                        });
                        let range = info.map_or(String::new(), |i| describe_range(&i.values));
                        ui.label(RichText::new(range).size(12.0).color(DIM));

                        ui.add_space(14.0);
                        ui.vertical_centered(|ui| {
                            let btn = egui::Button::new(RichText::new("Apply").size(16.0).color(Color32::WHITE))
                                .fill(ACCENT)
                                .min_size(egui::vec2(140.0, 34.0));
                            if ui.add(btn).clicked() {
                                self.target = snap(values, self.target);
                                let _ = self.tx.send(Cmd::Apply(self.target));
                            }
                        });
                    });
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    if let Some(e) = &snap_state.error {
                        ui.label(RichText::new(e).size(12.0).color(ERR));
                    }
                    let dot = if info.is_some() { ACCENT } else { ERR };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("●").size(12.0).color(dot));
                        ui.label(RichText::new(&snap_state.status).size(12.0).color(DIM));
                    });
                });
            });
    }
}

fn apply_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = BG;
    v.extreme_bg_color = BG;
    v.faint_bg_color = BG;
    v.code_bg_color = BG;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0, TEXT);

    let grey = Color32::from_gray(0x3a);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.bg_fill = BG;
        w.weak_bg_fill = BG;
        w.bg_stroke = Stroke::new(1.0, grey);
        w.fg_stroke = Stroke::new(1.0, TEXT);
    }
    v.widgets.noninteractive.bg_stroke = Stroke::NONE;
    // Slider rail and handle
    v.widgets.inactive.bg_fill = grey;
    v.widgets.hovered.bg_fill = ACCENT;
    v.widgets.active.bg_fill = ACCENT;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    ctx.set_visuals(v);
}

/// `--list`: print every mouse found. `--set <n> <dpi>`: set mouse n (from --list).
fn cli(args: &[String]) -> Result<(), String> {
    let mut api = hidapi::HidApi::new().map_err(|e| e.to_string())?;
    let nodes = hidpp::candidate_nodes(&mut api);
    let (mice, errors) = hidpp::discover(&api, &nodes);
    for e in &errors {
        eprintln!("warning: {e}");
    }
    if mice.is_empty() {
        return Err("no Logitech mouse with adjustable DPI found".into());
    }
    if args[1] == "--set" {
        let n: usize = args.get(2).and_then(|s| s.parse().ok()).ok_or("usage: --set <n> <dpi>")?;
        let dpi: u16 = args.get(3).and_then(|s| s.parse().ok()).ok_or("usage: --set <n> <dpi>")?;
        let m = mice.get(n.wrapping_sub(1)).ok_or(format!("no mouse {n}"))?;
        m.set_dpi(dpi).map_err(|e| e.to_string())?;
    }
    for (i, m) in mice.iter().enumerate() {
        let idx = if m.index == 0xff { String::new() } else { format!(" slot {}", m.index) };
        match m.read_info() {
            Ok(info) => println!(
                "{}. {} ({}{idx}): {} DPI, supports {}",
                i + 1,
                m.name,
                m.path,
                info.current,
                describe_range(&info.values)
            ),
            Err(e) => println!("{}. {} ({}{idx}): {e}", i + 1, m.name, m.path),
        }
    }
    Ok(())
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if matches!(args.get(1).map(String::as_str), Some("--list" | "--set")) {
        if let Err(e) = cli(&args) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Logitech DPI")
            .with_app_id("logitech-dpi")
            .with_inner_size([380.0, 320.0])
            .with_min_inner_size([320.0, 280.0]),
        ..Default::default()
    };
    eframe::run_native("Logitech DPI", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
