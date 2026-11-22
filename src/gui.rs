//! Desktop app: setup wizard, dashboard with AI insights, one-click "Fix everything".

use crate::brain::Level;
use crate::config::Config;
use crate::engine::{self, Snapshot};
use crate::i18n::{self, t, tf, Lang};
use crate::util::{human, remove, LOG_LINES};
use eframe::egui::{
    self, vec2, Align, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, Sense, Stroke, TextureHandle,
};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");
/// Noto Sans Thai (SIL Open Font License) — Thai text on every OS.
const THAI_FONT: &[u8] = include_bytes!("../assets/fonts/NotoSansThai-Regular.ttf");

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
const AI: Color32 = Color32::from_rgb(0x9B, 0x8C, 0xFF);

pub fn run() -> eframe::Result {
    let icon = eframe::icon_data::from_png_bytes(LOGO_PNG).expect("logo");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("rtidy")
            .with_inner_size([1000.0, 740.0])
            .with_min_inner_size([780.0, 580.0])
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native("rtidy", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

enum Msg {
    Snapshot(Box<Snapshot>),
    Done(String, bool),
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Insights,
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
        i18n::set(cfg.lang);
        let mut app = Self {
            saved_cfg: cfg.clone(),
            tab: if cfg.smart { Tab::Insights } else { Tab::Projects },
            cfg,
            snap: None,
            busy: None,
            rx: None,
            log: Vec::new(),
            toast: None,
            logo,
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
        self.spawn(ctx, t("Scanning your projects…"), |cfg| Msg::Snapshot(Box::new(engine::snapshot(&cfg))));
    }

    fn fix_all(&mut self, ctx: &egui::Context) {
        self.saved_cfg = self.cfg.clone();
        self.first_run = false;
        self.spawn(ctx, t("Tidying up…"), |cfg| match engine::fix_all(&cfg) {
            Ok(r) if r.warnings.is_empty() => Msg::Done(tf("All tidy! Freed {}", &[&human(r.freed)]), true),
            Ok(r) => Msg::Done(tf("Freed {}, but: {}", &[&human(r.freed), &r.warnings.join("; ")]), false),
            Err(e) => Msg::Done(tf("Something went wrong: {}", &[&e.to_string()]), false),
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Ok(mut v) = LOG_LINES.lock() {
            self.log.append(&mut v);
        }
        let Some(rx) = &self.rx else { return };
        match rx.try_recv() {
            Ok(Msg::Snapshot(s)) => {
                // the scheduled run may have moved the clean-up time (smart schedule)
                if let Some(at) = &s.auto_at {
                    self.cfg.auto_at = at.clone();
                    self.saved_cfg.auto_at = at.clone();
                }
                self.snap = Some(*s);
                self.busy = None;
                self.rx = None;
            }
            Ok(Msg::Done(text, ok)) => {
                self.last_ok = ok;
                self.last_done = Some(text.clone());
                self.toast = Some((text, Instant::now()));
                self.busy = None;
                self.rx = None;
                self.refresh(ctx);
            }
            Err(_) => {}
        }
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn set_lang(&mut self, lang: Lang) {
        i18n::set(lang);
        self.cfg.lang = lang;
        self.saved_cfg.lang = lang;
        let mut saved = Config::load();
        saved.lang = lang;
        if Config::path().is_ok_and(|p| p.exists()) {
            let _ = saved.save();
        }
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
                .show(ctx, |ui| self.wizard_ui(ui));
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
                        Tab::Insights => self.insights(ui),
                        Tab::Projects => self.projects(ui, ctx),
                        Tab::Settings => self.settings(ui, ctx),
                        Tab::Activity => self.activity(ui),
                    }
                });
            });

        self.draw_toast(ctx);
    }
}

// ---------------- dashboard ----------------

impl App {
    fn header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(self.logo.id(), [40.0, 40.0])));
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.add_space(1.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("rtidy").size(22.0).strong().color(TEXT));
                    if self.cfg.smart {
                        chip(ui, t("✨ Smart mode"), AI);
                    }
                });
                ui.label(RichText::new(t("Keeps your Rust target folders tidy")).size(13.0).color(MUTED));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let r = ui.add_enabled(self.busy.is_none(), ghost_button(t("⟳  Rescan")));
                if r.clicked() {
                    self.refresh(ctx);
                }
                ui.add_space(6.0);
                if let Some(l) = lang_switch(ui, self.cfg.lang) {
                    self.set_lang(l);
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
                            ui.label(RichText::new(t("This can take a moment on big folders.")).color(MUTED));
                        }
                        (None, Some(s)) => {
                            let issues = s.issues();
                            if issues.is_empty() {
                                ui.label(RichText::new(t("All tidy ✔")).size(26.0).strong().color(GOOD));
                                ui.label(
                                    RichText::new(t("Shared target folder, lean builds and daily clean-up are all on."))
                                        .color(MUTED),
                                );
                            } else {
                                ui.label(RichText::new(t("Up to")).color(MUTED));
                                ui.label(RichText::new(human(s.reclaimable())).size(40.0).strong().color(TEXT));
                                ui.label(RichText::new(t("can be freed")).color(MUTED));
                                ui.add_space(8.0);
                                for i in issues.iter().take(4) {
                                    ui.label(RichText::new(format!("•  {i}")).color(WARN));
                                }
                            }
                            if let Some(w) = s.insights.iter().find(|i| i.level == Level::Warn) {
                                ui.add_space(4.0);
                                ui.label(RichText::new(format!("✨  {}", w.title)).color(AI));
                            }
                        }
                        (None, None) => {}
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let enabled = self.busy.is_none() && !self.cfg.roots.is_empty();
                    let btn = egui::Button::new(RichText::new(t("✨  Fix everything")).size(18.0).strong().color(Color32::WHITE))
                        .fill(if enabled { ACCENT } else { ACCENT_DIM })
                        .corner_radius(14)
                        .min_size(vec2(220.0, 56.0));
                    if ui
                        .add_enabled(enabled, btn)
                        .on_hover_text(t("Share one target folder across projects, build dependencies lean, delete leftover target/ folders, trim caches and turn on daily clean-up."))
                        .clicked()
                    {
                        self.fix_all(ctx);
                    }
                });
            });
        });
    }

    fn stats(&self, ui: &mut egui::Ui) {
        let s = self.snap.clone().unwrap_or_default();
        let w = (ui.available_width() - 3.0 * 12.0) / 4.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let n = s.targets.iter().filter(|t| !t.keep).count();
            stat(ui, w, t("Leftover targets"), &human(s.leftover()), &tf("{} folders", &[&n.to_string()]), n > 0);
            stat(
                ui,
                w,
                t("Shared target"),
                &if s.shared.is_some() { human(s.shared_size) } else { t("Off").into() },
                &if s.shared.is_some() {
                    tf("cap {} GB", &[&self.cfg.max_size_gb.to_string()])
                } else {
                    t("not set up").into()
                },
                s.shared.is_none(),
            );
            stat(ui, w, t("Cargo cache"), &human(s.cache_size), t("~/.cargo downloads"), false);
            let sub = match s.days_to_full {
                Some(d) if d < 60.0 => tf("full in ~{} days", &[&format!("{d:.0}")]),
                _ => tf("{} toolchains", &[&s.toolchains.len().to_string()]),
            };
            stat(
                ui,
                w,
                t("Free disk"),
                &s.free.map(human).unwrap_or("–".into()),
                &sub,
                s.free.is_some_and(|f| f < 20 << 30) || s.days_to_full.is_some_and(|d| d < 21.0),
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
                    self.spawn(ctx, t("Updating schedule…"), |cfg| match engine::apply_schedule(&cfg) {
                        Ok(()) if cfg.auto_enabled => Msg::Done(tf("Daily clean-up on ({})", &[&cfg.auto_at]), true),
                        Ok(()) => Msg::Done(t("Daily clean-up off").into(), true),
                        Err(e) => Msg::Done(tf("Couldn't change schedule: {}", &[&e.to_string()]), false),
                    });
                }
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(t("Automatic daily clean-up")).size(15.0).strong().color(TEXT));
                    let smart = if self.cfg.smart_schedule && self.cfg.smart { t(" · time picked by AI") } else { "" };
                    let sub = match (scheduled, self.snap.as_ref().and_then(|s| s.last_run.clone())) {
                        (true, Some(l)) => tf("Every day at {}{} · last: {}", &[&self.cfg.auto_at, smart, &short_log(&l)]),
                        (true, None) => tf("Every day at {}{} · hasn't run yet", &[&self.cfg.auto_at, smart]),
                        (false, _) => t("Off — turn on to keep things tidy without thinking about it").into(),
                    };
                    ui.label(RichText::new(sub).color(MUTED));
                });
            });
        });
    }

    fn tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (tab, name) in [
                (Tab::Insights, t("✨ Insights")),
                (Tab::Projects, t("Projects")),
                (Tab::Settings, t("Settings")),
                (Tab::Activity, t("Activity")),
            ] {
                let sel = self.tab == tab;
                let b = egui::Button::new(RichText::new(name).size(14.0).color(if sel { TEXT } else { MUTED }))
                    .fill(if sel { CARD_HI } else { Color32::TRANSPARENT })
                    .stroke(Stroke::NONE)
                    .corner_radius(10)
                    .min_size(vec2(104.0, 32.0));
                if ui.add(b).clicked() {
                    self.tab = tab;
                }
            }
        });
    }

    fn insights(&mut self, ui: &mut egui::Ui) {
        if !self.cfg.smart {
            empty(ui, t("Smart mode is off. Turn it on in Settings to get AI insights."));
            return;
        }
        let Some(s) = self.snap.clone() else { return };
        // summary strip
        card(CARD).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (label, color) = match s.pressure {
                    0 => (t("Relaxed"), GOOD),
                    1 => (t("Getting tight"), WARN),
                    _ => (t("Critical"), Color32::from_rgb(0xFF, 0x5A, 0x5A)),
                };
                mini(ui, t("Disk pressure"), label, color);
                ui.add_space(28.0);
                mini(ui, t("Outcomes learned"), &s.learned.to_string(), TEXT);
                ui.add_space(28.0);
                let acc = if s.learned >= 30 { format!("{:.0}%", s.accuracy * 100.0) } else { t("learning…").into() };
                mini(ui, t("Prediction accuracy"), &acc, TEXT);
                ui.add_space(28.0);
                mini(ui, t("Unlikely to be reused"), &human(s.stale_artifacts), TEXT);
            });
        });
        ui.add_space(10.0);
        for ins in &s.insights {
            let color = match ins.level {
                Level::Good => GOOD,
                Level::Info => AI,
                Level::Warn => WARN,
            };
            card(CARD).inner_margin(Margin { left: 0, right: 16, top: 14, bottom: 14 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(4.0, 40.0), Sense::hover());
                    ui.painter().rect_filled(r, 2.0, color);
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&ins.title).size(15.0).strong().color(TEXT));
                        ui.label(RichText::new(&ins.detail).size(13.0).color(MUTED));
                    });
                });
            });
            ui.add_space(8.0);
        }
        ui.label(
            RichText::new(t("How it works: a small model learns which build files you reuse and how fast your disk fills. It runs on this computer in milliseconds; nothing is uploaded."))
                .size(12.0)
                .color(MUTED),
        );
    }

    fn projects(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(s) = self.snap.clone() else { return };
        if s.targets.is_empty() {
            empty(ui, t("No per-project target/ folders found. 🎉"));
            return;
        }
        let max = s.targets.iter().map(|t| t.size).max().unwrap_or(1).max(1);
        let mut changed = false;
        card(CARD).inner_margin(Margin::same(8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (i, row) in s.targets.iter().enumerate() {
                if i > 0 {
                    ui.add(egui::Separator::default().spacing(2.0));
                }
                ui.horizontal(|ui| {
                    ui.set_min_height(40.0);
                    ui.add_space(8.0);
                    let project = row.path.parent().unwrap_or(&row.path);
                    let name = project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - 540.0).max(150.0));
                        ui.label(RichText::new(&name).strong().color(TEXT));
                        ui.label(RichText::new(project.display().to_string()).size(12.0).color(MUTED))
                            .on_hover_text(row.path.display().to_string());
                    });
                    let (rect, _) = ui.allocate_exact_size(vec2(100.0, 6.0), Sense::hover());
                    ui.painter().rect_filled(rect, 3.0, LINE);
                    let mut fill = rect;
                    fill.set_width(rect.width() * (row.size as f32 / max as f32).max(0.03));
                    ui.painter().rect_filled(fill, 3.0, if row.keep { MUTED } else { ACCENT });
                    ui.add_sized([70.0, 20.0], egui::Label::new(RichText::new(human(row.size)).color(TEXT)));
                    ui.add_sized(
                        [62.0, 20.0],
                        egui::Label::new(RichText::new(tf("{}d ago", &[&row.age_days.to_string()])).color(MUTED)),
                    );
                    match row.reuse {
                        Some(p) if self.cfg.smart => {
                            let c = if p < 0.15 { MUTED } else if p < 0.5 { WARN } else { GOOD };
                            ui.add_sized([70.0, 20.0], egui::Label::new(RichText::new(format!("✨ {:.0}%", p * 100.0)).color(c)))
                                .on_hover_text(t("AI: chance you'll build this project again in the next 14 days"));
                        }
                        _ => {
                            ui.add_space(78.0);
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(!row.keep && self.busy.is_none(), ghost_button(t("Delete"))).clicked() {
                            let freed = remove(&row.path, false);
                            self.toast(tf("Deleted {} target ({})", &[&name, &human(freed)]));
                            changed = true;
                        }
                        let mut keep = row.keep;
                        if ui.checkbox(&mut keep, t("Keep")).on_hover_text(t("Never clean this project")).changed() {
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
            section(ui, t("Language"), t("Language of the app"));
            if let Some(l) = lang_switch(ui, self.cfg.lang) {
                self.set_lang(l);
            }
            ui.add_space(16.0);

            section(ui, t("Smart mode (AI)"), t("Learns how you work and decides what to clean"));
            option_row(ui, &mut self.cfg.smart, t("Smart clean-up"), t("Remove what you're unlikely to reuse first; clean harder when the disk is filling up"));
            option_row(ui, &mut self.cfg.smart_schedule, t("Smart schedule"), t("Run the daily clean-up at the hour you build least"));
            ui.add_space(12.0);

            section(ui, t("Project folders"), t("Where rtidy looks for Rust projects"));
            let mut remove_idx = None;
            for (i, r) in self.cfg.roots.iter().enumerate() {
                ui.horizontal(|ui| {
                    dot(ui, MUTED);
                    ui.label(RichText::new(r.display().to_string()).color(TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(ghost_button(t("Remove"))).clicked() {
                            remove_idx = Some(i);
                        }
                    });
                });
            }
            if let Some(i) = remove_idx {
                self.cfg.roots.remove(i);
            }
            ui.horizontal(|ui| {
                if ui.add(ghost_button(t("+  Add folder…"))).clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        if !self.cfg.roots.contains(&p) {
                            self.cfg.roots.push(p);
                        }
                    }
                }
                if ui.add(ghost_button(t("Reset to detected"))).clicked() {
                    self.cfg.roots = crate::config::default_roots();
                }
            });
            self.path_input(ui);

            ui.add_space(16.0);
            section(ui, t("Clean-up rules"), t("Limits that always apply, with or without AI"));
            slider(ui, t("Delete project target/ not built for"), &mut self.cfg.days, 0..=180, t("days"));
            slider(ui, t("Shared folder: drop artifacts unused for"), &mut self.cfg.gc_days, 1..=90, t("days"));
            slider(ui, t("Shared folder size cap (0 = none)"), &mut self.cfg.max_size_gb, 0..=200, "GB");
            slider(ui, t("Cargo downloads older than (0 = keep)"), &mut self.cfg.cache_days, 0..=365, t("days"));
            option_row(ui, &mut self.cfg.lean, t("Lean builds"), t("Dependencies without debug info (your own code keeps it)"));
            ui.horizontal(|ui| {
                ui.label(RichText::new(t("Daily clean-up time")).color(TEXT));
                ui.add_enabled(
                    !(self.cfg.smart && self.cfg.smart_schedule),
                    egui::TextEdit::singleline(&mut self.cfg.auto_at).desired_width(60.0),
                );
            });

            ui.add_space(14.0);
            let dirty = self.cfg != self.saved_cfg;
            ui.horizontal(|ui| {
                let b = egui::Button::new(RichText::new(t("Save")).strong().color(Color32::WHITE))
                    .fill(if dirty { ACCENT } else { ACCENT_DIM })
                    .corner_radius(10)
                    .min_size(vec2(110.0, 34.0));
                if ui.add_enabled(dirty, b).clicked() {
                    match self.cfg.save() {
                        Ok(()) => {
                            self.saved_cfg = self.cfg.clone();
                            self.toast(t("Settings saved"));
                            if self.snap.as_ref().is_some_and(|s| s.scheduled) {
                                let _ = engine::apply_schedule(&self.cfg);
                            }
                            self.refresh(ctx);
                        }
                        Err(e) => self.toast(tf("Couldn't save: {}", &[&e.to_string()])),
                    }
                }
                if dirty && ui.add(ghost_button(t("Discard"))).clicked() {
                    self.cfg = self.saved_cfg.clone();
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(ghost_button(t("Run setup wizard again"))).clicked() {
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
                ui.label(RichText::new(t("Nothing yet this session. Scheduled runs are logged to:")).color(MUTED));
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

    /// Type-a-path fallback (some Linux desktops have no folder-picker portal).
    fn path_input(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let te = egui::TextEdit::singleline(&mut self.add_path)
                .hint_text(t("…or type a folder path"))
                .desired_width(320.0);
            let r = ui.add(te);
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.add(ghost_button(t("Add"))).clicked() || enter) && !self.add_path.trim().is_empty() {
                let raw = self.add_path.trim();
                let p = match raw.strip_prefix('~') {
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
                    _ => self.toast(t("That folder doesn't exist")),
                }
            }
        });
    }
}

// ---------------- setup wizard ----------------

impl App {
    fn wizard_ui(&mut self, ui: &mut egui::Ui) {
        let step = self.wizard.as_ref().map_or(0, |w| w.step);
        let width = ui.available_width().min(760.0);
        ui.vertical_centered(|ui| {
            ui.set_max_width(width);
            ui.horizontal(|ui| {
                let seg = (width - 4.0 * 8.0) / 5.0;
                for (i, name) in STEPS.iter().enumerate() {
                    ui.vertical(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(seg, 4.0), Sense::hover());
                        ui.painter().rect_filled(r, 2.0, if i <= step { ACCENT } else { LINE });
                        ui.label(RichText::new(t(name)).size(12.0).color(if i == step { TEXT } else { MUTED }));
                    });
                }
            });
            ui.add_space(22.0);
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                card(CARD).inner_margin(Margin::same(28)).show(ui, |ui| {
                    ui.set_width(width - 58.0);
                    ui.vertical(|ui| match step {
                        0 => self.w_welcome(ui),
                        1 => self.w_folders(ui),
                        2 => self.w_prefs(ui),
                        3 => self.w_review(ui),
                        _ => self.w_done(ui),
                    });
                })
            });
        });
    }

    fn w_welcome(&mut self, ui: &mut egui::Ui) {
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            if let Some(l) = lang_switch(ui, self.cfg.lang) {
                self.set_lang(l);
            }
        });
        ui.vertical_centered(|ui| {
            ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(self.logo.id(), [96.0, 96.0])));
            ui.add_space(12.0);
            ui.label(RichText::new(t("Welcome to rtidy")).size(28.0).strong().color(TEXT));
            ui.label(RichText::new(t("Let's stop Rust builds from eating your disk.")).size(15.0).color(MUTED));
        });
        ui.add_space(22.0);
        for (icon, title, sub) in [
            ("📦", t("One shared target folder"), t("Every project builds into the same place — no more target/ in each repo, and shared dependencies compile once.")),
            ("⚡", t("Lean builds"), t("Dependencies skip debug info, which is often half the size. Your own code keeps it.")),
            ("✨", t("Smart, and it cleans itself"), t("A tiny on-device model learns what you reuse and when you work, then cleans quietly every day.")),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon).size(22.0).color(if icon == "✨" { AI } else { TEXT }));
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
        ui.label(RichText::new(t("Where are your projects?")).size(22.0).strong().color(TEXT));
        ui.label(RichText::new(t("We found these folders. rtidy only looks inside them.")).color(MUTED));
        ui.add_space(14.0);
        let mut remove_idx = None;
        egui::ScrollArea::vertical().id_salt("roots").max_height(200.0).show(ui, |ui| {
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
            ui.label(RichText::new(t("Add at least one folder to continue.")).color(WARN));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.add(ghost_button(t("+  Choose folder…"))).clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    if !self.cfg.roots.contains(&p) {
                        self.cfg.roots.push(p);
                    }
                }
            }
            if ui.add(ghost_button(t("Reset to detected"))).clicked() {
                self.cfg.roots = crate::config::default_roots();
            }
        });
        self.path_input(ui);
    }

    fn w_prefs(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new(t("How tidy should it be?")).size(22.0).strong().color(TEXT));
        ui.label(RichText::new(t("The defaults are good for most people.")).color(MUTED));
        ui.add_space(16.0);
        let mech = crate::schedule::mechanism();
        option_row(ui, &mut self.cfg.smart, t("Smart mode (AI)"), t("Learns what you reuse; cleans harder only when the disk is filling up"));
        option_row(ui, &mut self.cfg.lean, t("Lean builds"), t("Build dependencies without debug info"));
        option_row(
            ui,
            &mut self.cfg.auto_enabled,
            t("Clean up automatically every day"),
            &tf("Runs quietly in the background via {}", &[mech]),
        );
        if self.cfg.auto_enabled && crate::schedule::exe_looks_temporary() {
            ui.label(
                RichText::new(t("⚠  Move rtidy to your Applications folder first, then open it from there — otherwise the daily clean-up can't find it."))
                    .color(WARN),
            );
        }
        if self.cfg.auto_enabled {
            ui.horizontal(|ui| {
                ui.add_space(56.0);
                if self.cfg.smart {
                    ui.checkbox(&mut self.cfg.smart_schedule, t("Let AI pick the time"));
                }
                if !(self.cfg.smart && self.cfg.smart_schedule) {
                    ui.label(RichText::new(t("at")).color(MUTED));
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.auto_at).desired_width(60.0));
                }
                ui.label(RichText::new(t("if the computer is off, it runs at next start-up")).size(12.0).color(MUTED));
            });
            ui.add_space(8.0);
        }
        if crate::integrate::supported() {
            if let Some(w) = self.wizard.as_mut() {
                let label = if cfg!(windows) { t("Add to Start menu") } else { t("Add to app menu") };
                option_row(ui, &mut w.integrate, label, t("So you can open rtidy like any other app"));
            }
        }
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        slider(ui, t("Delete project target/ not built for"), &mut self.cfg.days, 0..=180, t("days"));
        slider(ui, t("Shared folder size cap (0 = none)"), &mut self.cfg.max_size_gb, 0..=200, "GB");
        slider(ui, t("Cargo downloads older than (0 = keep)"), &mut self.cfg.cache_days, 0..=365, t("days"));
    }

    fn w_review(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new(t("Ready to tidy up")).size(22.0).strong().color(TEXT));
        match (&self.busy, &self.snap) {
            (Some(label), _) => {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.add(egui::Spinner::new().size(36.0).color(ACCENT));
                    ui.label(RichText::new(label).color(MUTED));
                });
            }
            (None, Some(s)) => {
                ui.label(RichText::new(t("Here's what we found and what will happen:")).color(MUTED));
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 24.0) / 3.0;
                    let n = s.targets.iter().filter(|t| !t.keep).count();
                    stat(ui, w, t("target/ folders"), &n.to_string(), &human(s.leftover()), n > 0);
                    stat(ui, w, t("Cargo cache"), &human(s.cache_size), t("downloads"), false);
                    stat(ui, w, t("Up to"), &human(s.reclaimable()), t("can be freed"), false);
                });
                ui.add_space(14.0);
                let mut steps: Vec<String> = vec![if s.shared.is_some() {
                    t("Keep your shared target folder").into()
                } else {
                    t("Create one shared target folder for all projects").into()
                }];
                if self.cfg.lean && !s.lean {
                    steps.push(t("Turn on lean builds").into());
                }
                if s.targets.iter().any(|t| !t.keep) {
                    steps.push(t("Delete leftover target/ folders (they rebuild into the shared folder)").into());
                }
                if self.cfg.cache_days > 0 {
                    steps.push(tf("Remove cargo downloads older than {} days", &[&self.cfg.cache_days.to_string()]));
                }
                if self.cfg.smart {
                    steps.push(t("Start learning your build habits (on this computer only)").into());
                }
                if self.cfg.auto_enabled {
                    steps.push(if self.cfg.smart && self.cfg.smart_schedule {
                        t("Clean up every day at the time you build least").into()
                    } else {
                        tf("Clean up every day at {}", &[&self.cfg.auto_at])
                    });
                }
                for st in steps {
                    ui.label(RichText::new(format!("✔  {st}")).color(TEXT));
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(t("Your next build of each project will take a little longer once; after that it's faster."))
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
                ui.label(RichText::new(t("Deleting files can take a minute.")).color(MUTED));
            } else {
                if self.last_ok {
                    ui.label(RichText::new("✔").size(54.0).color(GOOD));
                    ui.label(RichText::new(t("You're all set")).size(28.0).strong().color(TEXT));
                } else {
                    ui.label(RichText::new("⚠").size(54.0).color(WARN));
                    ui.label(RichText::new(t("Almost there")).size(28.0).strong().color(TEXT));
                }
                if let Some(msg) = &self.last_done {
                    ui.label(RichText::new(msg).size(16.0).color(MUTED));
                }
                ui.add_space(10.0);
                let next = if self.last_ok {
                    t("rtidy will keep things tidy from now on. You can close this window.")
                } else {
                    t("You can turn on daily clean-up later from the dashboard.")
                };
                ui.label(RichText::new(next).color(MUTED));
            }
        });
    }

    fn w_nav(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, step: usize) {
        let busy = self.busy.is_some();
        ui.horizontal(|ui| {
            if step > 0 && step < 4 && ui.add_enabled(!busy, ghost_button(t("Back"))).clicked() {
                self.wizard.as_mut().unwrap().step -= 1;
            }
            if step == 0 && ui.add(egui::Button::new(RichText::new(t("Skip setup")).color(MUTED)).frame(false)).clicked() {
                self.wizard = None;
                return;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (label, enabled) = match step {
                    0 => (t("Get started"), true),
                    1 => (t("Next"), !self.cfg.roots.is_empty()),
                    2 => (t("Next"), true),
                    3 => (t("✨  Tidy up now"), !busy && self.snap.is_some()),
                    _ => (t("Open dashboard"), !busy),
                };
                let b = egui::Button::new(RichText::new(label).size(15.0).strong().color(Color32::WHITE))
                    .fill(if enabled { ACCENT } else { ACCENT_DIM })
                    .corner_radius(12)
                    .min_size(vec2(170.0, 44.0));
                if ui.add_enabled(enabled, b).clicked() {
                    match step {
                        2 => {
                            self.wizard.as_mut().unwrap().step = 3;
                            self.refresh(ctx);
                        }
                        3 => {
                            if self.wizard.as_ref().is_some_and(|w| w.integrate) {
                                if let Err(e) = crate::integrate::integrate() {
                                    self.toast(tf("Couldn't add to menu: {}", &[&e.to_string()]));
                                }
                            }
                            self.wizard.as_mut().unwrap().step = 4;
                            self.fix_all(ctx);
                        }
                        4 => {
                            self.wizard = None;
                            self.tab = if self.cfg.smart { Tab::Insights } else { Tab::Projects };
                        }
                        _ => self.wizard.as_mut().unwrap().step += 1,
                    }
                }
                if step == 3 && ui.add_enabled(!busy, ghost_button(t("Just save settings"))).clicked() {
                    let _ = self.cfg.save();
                    let _ = engine::apply_schedule(&self.cfg);
                    if self.wizard.as_ref().is_some_and(|w| w.integrate) {
                        let _ = crate::integrate::integrate();
                    }
                    self.saved_cfg = self.cfg.clone();
                    self.first_run = false;
                    self.wizard = None;
                    self.toast(t("Settings saved"));
                    self.refresh(ctx);
                }
            });
        });
    }
}

// ---------------- widgets & style ----------------

fn style(ctx: &egui::Context) {
    // Thai glyphs as a fallback after the default fonts, so both languages render everywhere.
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("noto_thai".into(), Arc::new(egui::FontData::from_static(THAI_FONT)));
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(fam).or_default().push("noto_thai".into());
    }
    ctx.set_fonts(fonts);

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

/// EN | ไทย segmented switch. Returns the newly chosen language.
fn lang_switch(ui: &mut egui::Ui, current: Lang) -> Option<Lang> {
    let mut out = None;
    Frame::new().fill(CARD_HI).corner_radius(10).inner_margin(Margin::same(3)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.horizontal(|ui| {
            for (l, name) in [(Lang::En, "EN"), (Lang::Th, "ไทย")] {
                let sel = l == current;
                let b = egui::Button::new(RichText::new(name).size(13.0).color(if sel { Color32::WHITE } else { MUTED }))
                    .fill(if sel { ACCENT } else { Color32::TRANSPARENT })
                    .stroke(Stroke::NONE)
                    .corner_radius(8)
                    .min_size(vec2(44.0, 26.0));
                if ui.add(b).clicked() && !sel {
                    out = Some(l);
                }
            }
        });
    });
    out
}

fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    Frame::new()
        .fill(color.linear_multiply(0.18))
        .corner_radius(8)
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.5).color(color));
        });
}

fn mini(ui: &mut egui::Ui, title: &str, value: &str, color: Color32) {
    ui.vertical(|ui| {
        ui.label(RichText::new(title).size(12.0).color(MUTED));
        ui.label(RichText::new(value).size(18.0).strong().color(color));
    });
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

/// iOS-style switch.
fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = vec2(46.0, 26.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let k = ui.ctx().animate_bool_responsive(resp.id, *on);
    ui.painter().rect_filled(rect, 13.0, if *on { ACCENT } else { LINE });
    let x = egui::lerp(rect.left() + 13.0..=rect.right() - 13.0, k);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), 10.0, Color32::WHITE);
    resp
}

fn short_log(l: &str) -> String {
    // "2026-11-21T16:09:52Z auto: freed 6.2 MB total" -> "2026-11-21 · freed 6.2 MB"
    let date = l.get(..10).unwrap_or("");
    let rest = l.split("auto:").nth(1).unwrap_or("").replace(" total", "");
    format!("{date} ·{rest}")
}
