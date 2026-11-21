//! Desktop app: dashboard + one-click "Fix everything".

use crate::config::Config;
use crate::engine::{self, Snapshot};
use crate::util::{human, remove, LOG_LINES};
use eframe::egui::{
    self, vec2, Align, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, Sense, Stroke, TextureHandle,
};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");

// palette
const BG: Color32 = Color32::from_rgb(0x0F, 0x11, 0x15);
const CARD: Color32 = Color32::from_rgb(0x18, 0x1B, 0x21);
const CARD_HI: Color32 = Color32::from_rgb(0x21, 0x25, 0x2D);
const LINE: Color32 = Color32::from_rgb(0x2A, 0x2F, 0x38);
const TEXT: Color32 = Color32::from_rgb(0xE8, 0xEA, 0xEE);
const MUTED: Color32 = Color32::from_rgb(0x8B, 0x93, 0xA1);
const ACCENT: Color32 = Color32::from_rgb(0xFF, 0x6A, 0x2B);
const ACCENT_DIM: Color32 = Color32::from_rgb(0x5A, 0x2A, 0x16);
const GOOD: Color32 = Color32::from_rgb(0x3D, 0xD6, 0x8C);
const WARN: Color32 = Color32::from_rgb(0xFF, 0xB0, 0x3A);

pub fn run() -> eframe::Result {
    let icon = eframe::icon_data::from_png_bytes(LOGO_PNG).expect("logo");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("rtidy")
            .with_inner_size([980.0, 720.0])
            .with_min_inner_size([760.0, 560.0])
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native("rtidy", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

enum Msg {
    Snapshot(Snapshot),
    Done(String),
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Projects,
    Settings,
    Activity,
}

struct App {
    cfg: Config,
    saved_cfg: Config,
    snap: Option<Snapshot>,
    busy: Option<String>,
    rx: Option<Receiver<Msg>>,
    log: Vec<String>,
    toast: Option<(String, Instant)>,
    logo: TextureHandle,
    tab: Tab,
    first_run: bool,
    wizard: Option<Wizard>,
    last_done: Option<String>,
    last_ok: bool,
    add_path: String,
}

/// First-run setup wizard state.
struct Wizard {
    step: usize,
    integrate: bool,
}

const STEPS: [&str; 5] = ["Welcome", "Folders", "Preferences", "Review", "Done"];

impl App {
    fn new(cc: &eframe::CreationContext) -> Self {
        style(&cc.egui_ctx);
        let icon = eframe::icon_data::from_png_bytes(LOGO_PNG).expect("logo");
        let img = egui::ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
        let logo = cc.egui_ctx.load_texture("logo", img, egui::TextureOptions::LINEAR);
        let first_run = Config::path().map(|p| !p.exists()).unwrap_or(true);
        let cfg = Config::load();
        let mut app = Self {
            saved_cfg: cfg.clone(),
            cfg,
            snap: None,
            busy: None,
            rx: None,
            log: Vec::new(),
            toast: None,
            logo,
            tab: Tab::Projects,
            first_run,
            wizard: first_run.then(|| Wizard { step: 0, integrate: !crate::integrate::is_integrated() }),
            last_done: None,
            last_ok: true,
            add_path: String::new(),
        };
        app.refresh(&cc.egui_ctx);
        app
    }

    fn spawn(&mut self, ctx: &egui::Context, label: &str, job: impl FnOnce(Config) -> Msg + Send + 'static) {
        if self.busy.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let cfg = self.cfg.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = job(cfg);
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
        self.rx = Some(rx);
        self.busy = Some(label.to_string());
    }

    fn refresh(&mut self, ctx: &egui::Context) {
        self.spawn(ctx, "Scanning your projects…", |cfg| Msg::Snapshot(engine::snapshot(&cfg)));
    }

    fn fix_all(&mut self, ctx: &egui::Context) {
        self.saved_cfg = self.cfg.clone();
        self.first_run = false;
        self.spawn(ctx, "Tidying up…", |cfg| {
            match engine::fix_all(&cfg) {
                Ok(r) if r.warnings.is_empty() => Msg::Done(format!("All tidy! Freed {}", human(r.freed))),
                Ok(r) => Msg::Done(format!("Freed {}, but: {}", human(r.freed), r.warnings.join("; "))),
                Err(e) => Msg::Done(format!("Something went wrong: {e}")),
            }
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Ok(mut v) = LOG_LINES.lock() {
            self.log.append(&mut v);
        }
        let Some(rx) = &self.rx else { return };
        match rx.try_recv() {
            Ok(Msg::Snapshot(s)) => {
                self.snap = Some(s);
                self.busy = None;
                self.rx = None;
            }
            Ok(Msg::Done(text)) => {
                self.last_ok = !(text.starts_with("Something") || text.contains(", but:") || text.starts_with("Couldn't"));
                self.last_done = Some(text.clone());
                self.toast = Some((text, Instant::now()));
                self.busy = None;
                self.rx = None;
                self.refresh(ctx);
            }
            Err(_) => {}
        }
    }

    fn toast(&mut self, t: impl Into<String>) {
        self.toast = Some((t.into(), Instant::now()));
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        if self.busy.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        if self.wizard.is_some() {
            let step = self.wizard.as_ref().map_or(0, |w| w.step);
            egui::TopBottomPanel::bottom("wiz_nav")
                .frame(Frame::new().fill(BG).inner_margin(Margin { left: 28, right: 28, top: 12, bottom: 24 }))
                .show_separator_line(false)
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.set_max_width(ui.available_width().min(760.0));
                        self.w_nav(ui, ctx, step);
                    });
                });
            egui::CentralPanel::default()
                .frame(Frame::new().fill(BG).inner_margin(Margin::same(28)))
                .show(ctx, |ui| self.wizard_ui(ui, ctx));
            self.draw_toast(ctx);
            return;
        }

        egui::TopBottomPanel::top("top")
            .frame(Frame::new().fill(BG).inner_margin(Margin { left: 24, right: 24, top: 18, bottom: 8 }))
            .show_separator_line(false)
            .show(ctx, |ui| self.header(ui, ctx));

        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin { left: 24, right: 24, top: 8, bottom: 16 }))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    self.hero(ui, ctx);
                    ui.add_space(14.0);
                    self.stats(ui);
                    ui.add_space(14.0);
                    self.automation(ui, ctx);
                    ui.add_space(18.0);
                    self.tabs(ui);
                    ui.add_space(10.0);
                    match self.tab {
                        Tab::Projects => self.projects(ui, ctx),
                        Tab::Settings => self.settings(ui, ctx),
                        Tab::Activity => self.activity(ui),
                    }
                });
            });

        self.draw_toast(ctx);
    }
}

// ---------------- sections ----------------

impl App {
    fn header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(self.logo.id(), [40.0, 40.0])));
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.add_space(1.0);
                ui.label(RichText::new("rtidy").size(22.0).strong().color(TEXT));
                ui.label(RichText::new("Keeps your Rust target folders tidy").size(13.0).color(MUTED));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let r = ui.add_enabled(self.busy.is_none(), ghost_button("⟳  Rescan"));
                if r.clicked() {
                    self.refresh(ctx);
                }
            });
        });
    }

    fn hero(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        card(CARD).inner_margin(Margin::same(22)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width((ui.available_width() - 250.0).max(200.0));
                    match (&self.busy, &self.snap) {
                        (Some(label), _) => {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(22.0).color(ACCENT));
                                ui.label(RichText::new(label).size(20.0).color(TEXT));
                            });
                            ui.label(RichText::new("This can take a moment on big folders.").color(MUTED));
                        }
                        (None, Some(s)) => {
                            let issues = s.issues();
                            if issues.is_empty() {
                                ui.label(RichText::new("All tidy ✔").size(26.0).strong().color(GOOD));
                                ui.label(
                                    RichText::new("Shared target folder, lean builds and daily clean-up are all on.")
                                        .color(MUTED),
                                );
                            } else {
                                ui.label(RichText::new("Up to").color(MUTED));
                                ui.label(
                                    RichText::new(human(s.reclaimable())).size(40.0).strong().color(TEXT),
                                );
                                ui.label(RichText::new("can be freed").color(MUTED));
                                ui.add_space(8.0);
                                for i in issues.iter().take(5) {
                                    ui.label(RichText::new(format!("•  {i}")).color(WARN));
                                }
                            }
                        }
                        (None, None) => {}
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let enabled = self.busy.is_none() && !self.cfg.roots.is_empty();
                    let btn = egui::Button::new(RichText::new("✨  Fix everything").size(18.0).strong().color(Color32::WHITE))
                        .fill(if enabled { ACCENT } else { ACCENT_DIM })
                        .corner_radius(14)
                        .min_size(vec2(220.0, 56.0));
                    if ui.add_enabled(enabled, btn).on_hover_text(
                        "Share one target folder across projects, build dependencies lean, delete leftover target/ folders, trim caches and turn on daily clean-up.",
                    ).clicked() {
                        self.fix_all(ctx);
                    }
                });
            });
            if self.first_run {
                ui.add_space(10.0);
                ui.label(
                    RichText::new(format!(
                        "First time? Check the folders in Settings ({} found), then press Fix everything. That's it.",
                        self.cfg.roots.len()
                    ))
                    .color(MUTED),
                );
            }
        });
    }

    fn stats(&self, ui: &mut egui::Ui) {
        let s = self.snap.clone().unwrap_or_default();
        let w = (ui.available_width() - 3.0 * 12.0) / 4.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let n = s.targets.iter().filter(|t| !t.keep).count();
            stat(ui, w, "Leftover targets", &human(s.leftover()), &format!("{n} folder{}", if n == 1 { "" } else { "s" }), n > 0);
            stat(
                ui,
                w,
                "Shared target",
                &if s.shared.is_some() { human(s.shared_size) } else { "Off".into() },
                &if s.shared.is_some() { format!("cap {} GB", self.cfg.max_size_gb) } else { "not set up".into() },
                s.shared.is_none(),
            );
            stat(ui, w, "Cargo cache", &human(s.cache_size), "~/.cargo downloads", false);
            stat(
                ui,
                w,
                "Free disk",
                &s.free.map(human).unwrap_or("–".into()),
                &format!("{} toolchains", s.toolchains.len()),
                s.free.is_some_and(|f| f < 20 << 30),
            );
        });
    }

    fn automation(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let scheduled = self.snap.as_ref().is_some_and(|s| s.scheduled);
        card(CARD).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let mut on = scheduled;
                if toggle(ui, &mut on).clicked() && self.busy.is_none() {
                    self.cfg.auto_enabled = on;
                    let _ = self.cfg.save();
                    self.saved_cfg = self.cfg.clone();
                    self.spawn(ctx, "Updating schedule…", |cfg| match engine::apply_schedule(&cfg) {
                        Ok(()) => Msg::Done(if cfg.auto_enabled {
                            format!("Daily clean-up on ({})", cfg.auto_at)
                        } else {
                            "Daily clean-up off".into()
                        }),
                        Err(e) => Msg::Done(format!("Couldn't change schedule: {e}")),
                    });
                }
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("Automatic daily clean-up").size(15.0).strong().color(TEXT));
                    let sub = match (scheduled, self.snap.as_ref().and_then(|s| s.last_run.clone())) {
                        (true, Some(l)) => format!("Every day at {} · last: {}", self.cfg.auto_at, short_log(&l)),
                        (true, None) => format!("Every day at {} · hasn't run yet", self.cfg.auto_at),
                        (false, _) => "Off — turn on to keep things tidy without thinking about it".into(),
                    };
                    ui.label(RichText::new(sub).color(MUTED));
                });
            });
        });
    }

    fn tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (t, name) in [(Tab::Projects, "Projects"), (Tab::Settings, "Settings"), (Tab::Activity, "Activity")] {
                let sel = self.tab == t;
                let b = egui::Button::new(RichText::new(name).size(14.0).color(if sel { TEXT } else { MUTED }))
                    .fill(if sel { CARD_HI } else { Color32::TRANSPARENT })
                    .stroke(Stroke::NONE)
                    .corner_radius(10)
                    .min_size(vec2(96.0, 32.0));
                if ui.add(b).clicked() {
                    self.tab = t;
                }
            }
        });
    }

    fn projects(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(s) = self.snap.clone() else { return };
        if s.targets.is_empty() {
            empty(ui, "No per-project target/ folders found. 🎉");
            return;
        }
        let max = s.targets.iter().map(|t| t.size).max().unwrap_or(1).max(1);
        let mut changed = false;
        card(CARD).inner_margin(Margin::same(8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (i, t) in s.targets.iter().enumerate() {
                if i > 0 {
                    ui.add(egui::Separator::default().spacing(2.0));
                }
                ui.horizontal(|ui| {
                    ui.set_min_height(40.0);
                    ui.add_space(8.0);
                    let project = t.path.parent().unwrap_or(&t.path);
                    let name = project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - 480.0).max(160.0));
                        ui.label(RichText::new(&name).strong().color(TEXT));
                        ui.label(RichText::new(project.display().to_string()).size(12.0).color(MUTED))
                            .on_hover_text(t.path.display().to_string());
                    });
                    // size bar
                    let (rect, _) = ui.allocate_exact_size(vec2(110.0, 6.0), Sense::hover());
                    ui.painter().rect_filled(rect, 3.0, LINE);
                    let mut fill = rect;
                    fill.set_width(rect.width() * (t.size as f32 / max as f32).max(0.03));
                    ui.painter().rect_filled(fill, 3.0, if t.keep { MUTED } else { ACCENT });
                    ui.add_sized([72.0, 20.0], egui::Label::new(RichText::new(human(t.size)).color(TEXT)));
                    ui.add_sized(
                        [60.0, 20.0],
                        egui::Label::new(RichText::new(format!("{}d ago", t.age_days)).color(MUTED)),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(!t.keep && self.busy.is_none(), ghost_button("Delete")).clicked() {
                            let freed = remove(&t.path, false);
                            self.toast(format!("Deleted {name} target ({})", human(freed)));
                            changed = true;
                        }
                        let mut keep = t.keep;
                        if ui.checkbox(&mut keep, "Keep").on_hover_text("Never clean this project").changed() {
                            let marker = project.join(crate::targets::KEEP_MARKER);
                            let _ = if keep { std::fs::write(&marker, "") } else { std::fs::remove_file(&marker) };
                            changed = true;
                        }
                    });
                });
            }
        });
        if changed {
            self.refresh(ctx);
        }
    }

    fn settings(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        card(CARD).inner_margin(Margin::same(18)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            section(ui, "Project folders", "Where rtidy looks for Rust projects");
            let mut remove_idx = None;
            for (i, r) in self.cfg.roots.iter().enumerate() {
                ui.horizontal(|ui| {
                    dot(ui, MUTED);
                    ui.label(RichText::new(r.display().to_string()).color(TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(ghost_button("Remove")).clicked() {
                            remove_idx = Some(i);
                        }
                    });
                });
            }
            if let Some(i) = remove_idx {
                self.cfg.roots.remove(i);
            }
            ui.horizontal(|ui| {
                if ui.add(ghost_button("+  Add folder…")).clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        if !self.cfg.roots.contains(&p) {
                            self.cfg.roots.push(p);
                        }
                    }
                }
                if ui.add(ghost_button("Reset to detected")).clicked() {
                    self.cfg.roots = crate::config::default_roots();
                }
            });
            self.path_input(ui);

            ui.add_space(16.0);
            section(ui, "Clean-up rules", "Used by Fix everything and the daily clean-up");
            slider(ui, "Delete project target/ not built for", &mut self.cfg.days, 0..=180, "days");
            slider(ui, "Shared folder: drop artifacts unused for", &mut self.cfg.gc_days, 1..=90, "days");
            slider(ui, "Shared folder size cap (0 = none)", &mut self.cfg.max_size_gb, 0..=200, "GB");
            slider(ui, "Cargo cache: drop downloads older than (0 = keep)", &mut self.cfg.cache_days, 0..=365, "days");
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.cfg.lean, "");
                ui.label(RichText::new("Lean builds — dependencies without debug info (your own code keeps it)").color(TEXT));
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("Daily clean-up time").color(TEXT));
                ui.add(egui::TextEdit::singleline(&mut self.cfg.auto_at).desired_width(60.0));
            });

            ui.add_space(14.0);
            let dirty = self.cfg != self.saved_cfg;
            ui.horizontal(|ui| {
                let b = egui::Button::new(RichText::new("Save").strong().color(Color32::WHITE))
                    .fill(if dirty { ACCENT } else { ACCENT_DIM })
                    .corner_radius(10)
                    .min_size(vec2(110.0, 34.0));
                if ui.add_enabled(dirty, b).clicked() {
                    match self.cfg.save() {
                        Ok(()) => {
                            self.saved_cfg = self.cfg.clone();
                            self.toast("Settings saved");
                            if self.snap.as_ref().is_some_and(|s| s.scheduled) {
                                let _ = engine::apply_schedule(&self.cfg);
                            }
                            self.refresh(ctx);
                        }
                        Err(e) => self.toast(format!("Couldn't save: {e}")),
                    }
                }
                if dirty && ui.add(ghost_button("Discard")).clicked() {
                    self.cfg = self.saved_cfg.clone();
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(ghost_button("Run setup wizard again")).clicked() {
                        self.wizard = Some(Wizard { step: 0, integrate: !crate::integrate::is_integrated() });
                    }
                });
            });
        });
    }

    fn activity(&self, ui: &mut egui::Ui) {
        card(CARD).inner_margin(Margin::same(14)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            if self.log.is_empty() {
                ui.label(RichText::new("Nothing yet this session. Scheduled runs are logged to:").color(MUTED));
                if let Ok(p) = crate::util::log_path() {
                    ui.label(RichText::new(p.display().to_string()).monospace().color(TEXT));
                }
            }
            for l in self.log.iter().rev().take(300) {
                ui.label(RichText::new(l).font(FontId::monospace(12.5)).color(TEXT));
            }
        });
    }

    fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some((text, at)) = &self.toast else { return };
        if at.elapsed() > Duration::from_secs(4) {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(200));
        egui::Area::new(egui::Id::new("toast"))
            .anchor(egui::Align2::CENTER_TOP, vec2(0.0, 18.0))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(CARD_HI)
                    .stroke(Stroke::new(1.0_f32, LINE))
                    .corner_radius(12)
                    .inner_margin(Margin::symmetric(18, 12))
                    .show(ui, |ui| {
                        ui.label(RichText::new(text.as_str()).color(TEXT).size(14.5));
                    });
            });
    }
}

// ---------------- widgets & style ----------------

fn style(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::dark());
    ctx.style_mut(|s| {
        s.spacing.item_spacing = vec2(10.0, 8.0);
        s.spacing.button_padding = vec2(14.0, 7.0);
        s.spacing.slider_width = 220.0;
        let v = &mut s.visuals;
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.extreme_bg_color = Color32::from_rgb(0x12, 0x14, 0x19);
        v.selection.bg_fill = ACCENT;
        v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
        v.hyperlink_color = ACCENT;
        let r = CornerRadius::same(10);
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.noninteractive] {
            w.corner_radius = r;
        }
        v.widgets.inactive.bg_fill = CARD_HI;
        v.widgets.inactive.weak_bg_fill = CARD_HI;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, LINE);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x2A, 0x2F, 0x38);
        v.widgets.hovered.bg_fill = Color32::from_rgb(0x2A, 0x2F, 0x38);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(14.5));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(14.0));
    });
}

fn card(fill: Color32) -> Frame {
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0_f32, LINE))
        .corner_radius(16)
        .inner_margin(Margin::same(16))
}

fn ghost_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).color(TEXT)).corner_radius(10)
}

fn stat(ui: &mut egui::Ui, w: f32, title: &str, value: &str, sub: &str, alert: bool) {
    card(CARD).show(ui, |ui| {
        ui.set_width(w - 34.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(title).size(12.5).color(MUTED));
            ui.label(RichText::new(value).size(24.0).strong().color(if alert { WARN } else { TEXT }));
            ui.label(RichText::new(sub).size(12.0).color(MUTED));
        });
    });
}

fn section(ui: &mut egui::Ui, title: &str, sub: &str) {
    ui.label(RichText::new(title).size(16.0).strong().color(TEXT));
    ui.label(RichText::new(sub).size(12.5).color(MUTED));
    ui.add_space(6.0);
}

fn slider(ui: &mut egui::Ui, label: &str, v: &mut u64, range: std::ops::RangeInclusive<u64>, unit: &str) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(vec2(340.0, 22.0), Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_width(340.0);
            ui.label(RichText::new(label).color(TEXT));
        });
        ui.add(egui::Slider::new(v, range).suffix(format!(" {unit}")));
    });
}

fn empty(ui: &mut egui::Ui, text: &str) {
    card(CARD).inner_margin(Margin::same(28)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| ui.label(RichText::new(text).size(15.0).color(MUTED)));
    });
}

/// iOS-style switch.
fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = vec2(46.0, 26.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let bg = if *on { ACCENT } else { LINE };
    ui.painter().rect_filled(rect, 13.0, bg);
    let x = egui::lerp(rect.left() + 13.0..=rect.right() - 13.0, t);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), 10.0, Color32::WHITE);
    resp
}

fn short_log(l: &str) -> String {
    // "2026-11-21T16:09:52Z auto: freed 6.2 MB total" -> "2026-11-21 · freed 6.2 MB"
    let date = l.get(..10).unwrap_or("");
    let rest = l.split("auto:").nth(1).unwrap_or("").replace(" total", "");
    format!("{date} ·{rest}")
}

// ---------------- setup wizard ----------------

impl App {
    /// Type-a-path fallback (some Linux desktops have no folder-picker portal).
    fn path_input(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let te = egui::TextEdit::singleline(&mut self.add_path)
                .hint_text("…or type a folder path")
                .desired_width(320.0);
            let r = ui.add(te);
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.add(ghost_button("Add")).clicked() || enter) && !self.add_path.trim().is_empty() {
                let raw = self.add_path.trim();
                let p = match raw.strip_prefix("~") {
                    Some(rest) => dirs::home_dir().unwrap_or_default().join(rest.trim_start_matches(['/', '\\'])),
                    None => std::path::PathBuf::from(raw),
                };
                match p.canonicalize() {
                    Ok(c) if c.is_dir() => {
                        if !self.cfg.roots.contains(&c) {
                            self.cfg.roots.push(c);
                        }
                        self.add_path.clear();
                    }
                    _ => self.toast("That folder doesn't exist"),
                }
            }
        });
    }

    fn wizard_ui(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let step = self.wizard.as_ref().map_or(0, |w| w.step);
        let width = ui.available_width().min(760.0);
        ui.vertical_centered(|ui| {
            ui.set_max_width(width);
            // progress
            ui.horizontal(|ui| {
                let seg = (width - 4.0 * 8.0) / 5.0;
                for (i, name) in STEPS.iter().enumerate() {
                    ui.vertical(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(seg, 4.0), Sense::hover());
                        ui.painter().rect_filled(r, 2.0, if i <= step { ACCENT } else { LINE });
                        ui.label(RichText::new(*name).size(12.0).color(if i == step { TEXT } else { MUTED }));
                    });
                }
            });
            ui.add_space(22.0);
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| card(CARD).inner_margin(Margin::same(28)).show(ui, |ui| {
                ui.set_width(width - 58.0);
                ui.vertical(|ui| match step {
                    0 => self.w_welcome(ui),
                    1 => self.w_folders(ui),
                    2 => self.w_prefs(ui),
                    3 => self.w_review(ui),
                    _ => self.w_done(ui),
                });
            }));
        });
    }

    fn w_welcome(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(10.0);
            ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(self.logo.id(), [96.0, 96.0])));
            ui.add_space(12.0);
            ui.label(RichText::new("Welcome to rtidy").size(28.0).strong().color(TEXT));
            ui.label(RichText::new("Let's stop Rust builds from eating your disk.").size(15.0).color(MUTED));
        });
        ui.add_space(22.0);
        for (icon, title, sub) in [
            ("📦", "One shared target folder", "Every project builds into the same place — no more target/ in each repo, and shared dependencies compile once."),
            ("⚡", "Lean builds", "Dependencies skip debug info, which is often half the size. Your own code keeps it."),
            ("⏰", "Cleans itself", "A small daily task trims old artifacts and download caches. Set it and forget it."),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon).size(22.0));
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).strong().color(TEXT));
                    ui.label(RichText::new(sub).size(13.0).color(MUTED));
                });
            });
            ui.add_space(8.0);
        }
    }

    fn w_folders(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Where are your projects?").size(22.0).strong().color(TEXT));
        ui.label(RichText::new("We found these folders. rtidy only looks inside them.").color(MUTED));
        ui.add_space(14.0);
        let mut remove_idx = None;
        egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
            for (i, r) in self.cfg.roots.iter().enumerate() {
                Frame::new().fill(CARD_HI).corner_radius(10).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        dot(ui, ACCENT);
                        ui.label(RichText::new(r.display().to_string()).color(TEXT));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add(egui::Button::new(RichText::new("×").size(18.0).color(MUTED)).frame(false)).clicked() {
                                remove_idx = Some(i);
                            }
                        });
                    });
                });
                ui.add_space(4.0);
            }
        });
        if let Some(i) = remove_idx {
            self.cfg.roots.remove(i);
        }
        if self.cfg.roots.is_empty() {
            ui.label(RichText::new("Add at least one folder to continue.").color(WARN));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.add(ghost_button("+  Choose folder…")).clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    if !self.cfg.roots.contains(&p) {
                        self.cfg.roots.push(p);
                    }
                }
            }
            if ui.add(ghost_button("Reset to detected")).clicked() {
                self.cfg.roots = crate::config::default_roots();
            }
        });
        self.path_input(ui);
    }

    fn w_prefs(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("How tidy should it be?").size(22.0).strong().color(TEXT));
        ui.label(RichText::new("The defaults are good for most people.").color(MUTED));
        ui.add_space(16.0);
        let mech = crate::schedule::mechanism();
        option_row(ui, &mut self.cfg.lean, "Lean builds", "Build dependencies without debug info");
        option_row(
            ui,
            &mut self.cfg.auto_enabled,
            "Clean up automatically every day",
            &format!("Runs quietly in the background via {mech}"),
        );
        if self.cfg.auto_enabled && crate::schedule::exe_looks_temporary() {
            ui.label(
                RichText::new("⚠  Move rtidy to your Applications folder first, then open it from there — otherwise the daily clean-up can't find it.")
                    .color(WARN),
            );
        }
        if self.cfg.auto_enabled {
            ui.horizontal(|ui| {
                ui.add_space(56.0);
                ui.label(RichText::new("at").color(MUTED));
                ui.add(egui::TextEdit::singleline(&mut self.cfg.auto_at).desired_width(60.0));
                ui.label(RichText::new("24-hour time · if the computer is off, it runs at next start-up").size(12.0).color(MUTED));
            });
        }
        if crate::integrate::supported() {
            if let Some(w) = self.wizard.as_mut() {
                let label = if cfg!(windows) { "Add to Start menu" } else { "Add to app menu" };
                option_row(ui, &mut w.integrate, label, "So you can open rtidy like any other app");
            }
        }
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
        slider(ui, "Delete project target/ not built for", &mut self.cfg.days, 0..=180, "days");
        slider(ui, "Shared folder size cap (0 = none)", &mut self.cfg.max_size_gb, 0..=200, "GB");
        slider(ui, "Cargo downloads older than (0 = keep)", &mut self.cfg.cache_days, 0..=365, "days");
    }

    fn w_review(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Ready to tidy up").size(22.0).strong().color(TEXT));
        match (&self.busy, &self.snap) {
            (Some(label), _) => {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.add(egui::Spinner::new().size(36.0).color(ACCENT));
                    ui.label(RichText::new(label).color(MUTED));
                });
            }
            (None, Some(s)) => {
                ui.label(RichText::new("Here's what we found and what will happen:").color(MUTED));
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 24.0) / 3.0;
                    let n = s.targets.iter().filter(|t| !t.keep).count();
                    stat(ui, w, "target/ folders", &n.to_string(), &human(s.leftover()), n > 0);
                    stat(ui, w, "Cargo cache", &human(s.cache_size), "downloads", false);
                    stat(ui, w, "Up to", &human(s.reclaimable()), "can be freed", false);
                });
                ui.add_space(14.0);
                let shared = if s.shared.is_some() { "Keep your shared target folder" } else { "Create one shared target folder for all projects" };
                let mut steps = vec![shared.to_string()];
                if self.cfg.lean && !s.lean {
                    steps.push("Turn on lean builds".into());
                }
                if s.targets.iter().any(|t| !t.keep) {
                    steps.push("Delete leftover target/ folders (they rebuild into the shared folder)".into());
                }
                if self.cfg.cache_days > 0 {
                    steps.push(format!("Remove cargo downloads older than {} days", self.cfg.cache_days));
                }
                if self.cfg.auto_enabled {
                    steps.push(format!("Clean up every day at {}", self.cfg.auto_at));
                }
                for st in steps {
                    ui.label(RichText::new(format!("✔  {st}")).color(TEXT));
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Your next build of each project will take a little longer once; after that it's faster.")
                        .size(12.5)
                        .color(MUTED),
                );
            }
            (None, None) => {}
        }
    }

    fn w_done(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(30.0);
            if let Some(label) = &self.busy {
                ui.add(egui::Spinner::new().size(44.0).color(ACCENT));
                ui.add_space(10.0);
                ui.label(RichText::new(label).size(18.0).color(TEXT));
                ui.label(RichText::new("Deleting files can take a minute.").color(MUTED));
            } else {
                if self.last_ok {
                    ui.label(RichText::new("✔").size(54.0).color(GOOD));
                    ui.label(RichText::new("You're all set").size(28.0).strong().color(TEXT));
                } else {
                    ui.label(RichText::new("⚠").size(54.0).color(WARN));
                    ui.label(RichText::new("Almost there").size(28.0).strong().color(TEXT));
                }
                if let Some(t) = &self.last_done {
                    ui.label(RichText::new(t).size(16.0).color(MUTED));
                }
                ui.add_space(10.0);
                let next = if self.last_ok {
                    "rtidy will keep things tidy from now on. You can close this window."
                } else {
                    "You can turn on daily clean-up later from the dashboard."
                };
                ui.label(RichText::new(next).color(MUTED));
            }
        });
    }

    fn w_nav(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, step: usize) {
        let busy = self.busy.is_some();
        ui.horizontal(|ui| {
            if step > 0 && step < 4 && ui.add_enabled(!busy, ghost_button("Back")).clicked() {
                self.wizard.as_mut().unwrap().step -= 1;
            }
            if step == 0 && ui.add(egui::Button::new(RichText::new("Skip setup").color(MUTED)).frame(false)).clicked() {
                self.wizard = None;
                return;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (label, enabled) = match step {
                    0 => ("Get started", true),
                    1 => ("Next", !self.cfg.roots.is_empty()),
                    2 => ("Next", crate::schedule::exe_path().is_ok()),
                    3 => ("✨  Tidy up now", !busy && self.snap.is_some()),
                    _ => ("Open dashboard", !busy),
                };
                let b = egui::Button::new(RichText::new(label).size(15.0).strong().color(Color32::WHITE))
                    .fill(if enabled { ACCENT } else { ACCENT_DIM })
                    .corner_radius(12)
                    .min_size(vec2(170.0, 44.0));
                if ui.add_enabled(enabled, b).clicked() {
                    match step {
                        2 => {
                            // re-scan with the chosen folders
                            self.wizard.as_mut().unwrap().step = 3;
                            self.refresh(ctx);
                        }
                        3 => {
                            if self.wizard.as_ref().is_some_and(|w| w.integrate) {
                                if let Err(e) = crate::integrate::integrate() {
                                    self.toast(format!("Couldn't add to menu: {e}"));
                                }
                            }
                            self.wizard.as_mut().unwrap().step = 4;
                            self.fix_all(ctx);
                        }
                        4 => self.wizard = None,
                        _ => self.wizard.as_mut().unwrap().step += 1,
                    }
                }
                if step == 3 && ui.add_enabled(!busy, ghost_button("Just save settings")).clicked() {
                    let _ = self.cfg.save();
                    let _ = engine::apply_schedule(&self.cfg);
                    if self.wizard.as_ref().is_some_and(|w| w.integrate) {
                        let _ = crate::integrate::integrate();
                    }
                    self.saved_cfg = self.cfg.clone();
                    self.first_run = false;
                    self.wizard = None;
                    self.toast("Settings saved");
                    self.refresh(ctx);
                }
            });
        });
    }
}

fn option_row(ui: &mut egui::Ui, v: &mut bool, title: &str, sub: &str) {
    ui.horizontal(|ui| {
        toggle(ui, v);
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(title).strong().color(TEXT));
            ui.label(RichText::new(sub).size(12.5).color(MUTED));
        });
    });
    ui.add_space(8.0);
}

fn dot(ui: &mut egui::Ui, color: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 3.5, color);
}
