//! Desktop app: setup wizard, dashboard with AI insights, one-click "Fix everything".

use crate::brain::Level;
use crate::config::Config;
use crate::engine::{self, Snapshot};
use crate::i18n::{self, t, tf, Lang};
use crate::util::{human, remove_known, LOG_LINES};
use eframe::egui::{
    self, vec2, Align, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, Sense, Stroke, TextureHandle,
};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, LazyLock};
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
    /// Folders found by searching the computer (see `crate::discover`).
    Found(Vec<std::path::PathBuf>),
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
    /// The Folders step searches the computer by itself the first time it is shown
    /// with nothing to offer; this keeps it to one attempt, so a machine with no Rust
    /// on it does not re-scan on every repaint.
    searched: bool,
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
            wizard: first_run.then(|| Wizard { step: 0, integrate: !crate::integrate::is_integrated(), searched: false }),
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

    /// Search the whole computer for Rust projects, off the UI thread.
    ///
    /// Bounded to a few seconds so a huge or slow filesystem can never hang the app;
    /// whatever has been found by then is what comes back.
    fn search_computer(&mut self, ctx: &egui::Context) {
        self.spawn(ctx, t("Searching this computer…"), |_cfg| {
            Msg::Found(crate::discover::discover_roots(Duration::from_secs(20)))
        });
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
            Ok(Msg::Found(found)) => {
                let before = self.cfg.roots.len();
                for f in found {
                    if !self.cfg.roots.contains(&f) {
                        self.cfg.roots.push(f);
                    }
                }
                let added = self.cfg.roots.len() - before;
                self.toast(match added {
                    0 => t("No new project folders found").to_string(),
                    1 => t("Added 1 project folder").to_string(),
                    n => tf("Added {} project folders", &[&n.to_string()]),
                });
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
    // egui 0.34 turned `App::update(&Context)` into `App::ui(&mut Ui)`: panels are now
    // nested inside a root `Ui` with `show_inside` rather than attached to the context.
    // The `Context` is still needed for repaint requests and the toast overlay, and is
    // an `Arc` handle, so cloning it once per frame is free.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        if self.busy.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        if self.wizard.is_some() {
            let step = self.wizard.as_ref().map_or(0, |w| w.step);
            egui::Panel::bottom("wiz_nav")
                .frame(Frame::new().fill(BG).inner_margin(Margin { left: 28, right: 28, top: 12, bottom: 24 }))
                .show_separator_line(false)
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.set_max_width(ui.available_width().min(760.0));
                        self.w_nav(ui, &ctx, step);
                    });
                });
            egui::CentralPanel::default()
                .frame(Frame::new().fill(BG).inner_margin(Margin::same(28)))
                .show(ui, |ui| self.wizard_ui(ui, &ctx));
            self.draw_toast(&ctx);
            return;
        }

        egui::Panel::top("top")
            .frame(Frame::new().fill(BG).inner_margin(Margin { left: 24, right: 24, top: 18, bottom: 8 }))
            .show_separator_line(false)
            .show(ui, |ui| self.header(ui, &ctx));

        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin { left: 24, right: 24, top: 8, bottom: 16 }))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    self.hero(ui, &ctx);
                    ui.add_space(14.0);
                    self.stats(ui);
                    ui.add_space(14.0);
                    self.automation(ui, &ctx);
                    ui.add_space(18.0);
                    self.tabs(ui);
                    ui.add_space(10.0);
                    match self.tab {
                        Tab::Insights => self.insights(ui),
                        Tab::Projects => self.projects(ui, &ctx),
                        Tab::Settings => self.settings(ui, &ctx),
                        Tab::Activity => self.activity(ui),
                    }
                });
            });

        self.draw_toast(&ctx);
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
                        chip_icon(ui, Icon::Sparkle, t("Smart mode"), AI);
                    }
                });
                ui.label(RichText::new(t("Keeps your Rust target folders tidy")).size(13.0).color(MUTED));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let r = icon_button(
                    ui,
                    self.busy.is_none(),
                    Icon::Refresh,
                    t("Rescan"),
                    14.0,
                    Color32::TRANSPARENT,
                    LINE,
                    if self.busy.is_none() { TEXT } else { MUTED },
                    vec2(0.0, 32.0),
                );
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
                            ui.add_space(10.0);
                            sweep_bar(ui, ui.available_width().min(360.0));
                        }
                        (None, Some(s)) => {
                            let issues = s.issues();
                            if issues.is_empty() {
                                icon_label(ui, Icon::Check, 24.0, GOOD, RichText::new(t("All tidy")).size(26.0).strong().color(GOOD));
                                ui.label(
                                    RichText::new(t("Shared target folder, lean builds and daily clean-up are all on."))
                                        .color(MUTED),
                                );
                            } else {
                                ui.label(RichText::new(t("Up to")).color(MUTED));
                                let shown = anim_bytes(ctx, "reclaimable", s.reclaimable());
                                ui.label(RichText::new(human(shown)).size(40.0).strong().color(TEXT));
                                ui.label(RichText::new(t("can be freed")).color(MUTED));
                                ui.add_space(8.0);
                                for i in issues.iter().take(4) {
                                    ui.label(RichText::new(format!("•  {i}")).color(WARN));
                                }
                            }
                            if let Some(w) = s.insights.iter().find(|i| i.level() == Level::Warn) {
                                ui.add_space(4.0);
                                icon_label(ui, Icon::Sparkle, 15.0, AI, RichText::new(w.title()).color(AI));
                            }
                        }
                        (None, None) => {}
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let enabled = self.busy.is_none() && !self.cfg.roots.is_empty();
                    if icon_button(
                        ui,
                        enabled,
                        Icon::Sparkle,
                        t("Fix everything"),
                        18.0,
                        if enabled { ACCENT } else { ACCENT_DIM },
                        Color32::TRANSPARENT,
                        Color32::WHITE,
                        vec2(220.0, 56.0),
                    )
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
        // One shared empty snapshot stands in until the first scan lands, so the row
        // keeps its place in the layout without building a fresh default each repaint.
        static EMPTY: LazyLock<Snapshot> = LazyLock::new(Snapshot::default);
        let s = self.snap.as_ref().unwrap_or(&EMPTY);
        let ctx = ui.ctx().clone();
        let w = (ui.available_width() - 3.0 * 12.0) / 4.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let n = s.targets.iter().filter(|t| !t.keep).count();
            let leftover = anim_bytes(&ctx, "leftover", s.leftover());
            let n_shown = anim_count(&ctx, "leftover_n", n as u64);
            stat(ui, w, t("Leftover targets"), &human(leftover), &tf("{} folders", &[&n_shown.to_string()]), n > 0);
            stat(
                ui,
                w,
                t("Shared target"),
                &if s.shared.is_some() { human(anim_bytes(&ctx, "shared", s.shared_size)) } else { t("Off").into() },
                &if s.shared.is_some() {
                    tf("cap {} GB", &[&self.cfg.max_size_gb.to_string()])
                } else {
                    t("not set up").into()
                },
                s.shared.is_none(),
            );
            stat(ui, w, t("Cargo cache"), &human(anim_bytes(&ctx, "cache", s.cache_size)), t("~/.cargo downloads"), false);
            let sub = match s.days_to_full {
                Some(d) if d < 60.0 => tf("full in ~{} days", &[&format!("{d:.0}")]),
                _ => tf("{} toolchains", &[&s.toolchains.len().to_string()]),
            };
            stat(
                ui,
                w,
                t("Free disk"),
                &s.free.map(|f| human(anim_bytes(&ctx, "free", f))).unwrap_or("–".into()),
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
                (Tab::Insights, t("Insights")),
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

    fn insights(&self, ui: &mut egui::Ui) {
        if !self.cfg.smart {
            empty(ui, t("Smart mode is off. Turn it on in Settings to get AI insights."));
            return;
        }
        let Some(s) = self.snap.as_ref() else { return };
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
            let color = match ins.level() {
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
                        ui.label(RichText::new(ins.title()).size(15.0).strong().color(TEXT));
                        ui.label(RichText::new(ins.detail()).size(13.0).color(MUTED));
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
        // Moved out of `self` for the frame rather than cloned: the row list can hold
        // hundreds of paths and it was being deep-copied on every repaint.
        let Some(s) = self.snap.take() else { return };
        let changed = self.project_rows(ui, &s);
        self.snap = Some(s);
        if changed {
            self.refresh(ctx);
        }
    }

    /// Returns whether something on disk changed and the snapshot needs refreshing.
    fn project_rows(&mut self, ui: &mut egui::Ui, s: &Snapshot) -> bool {
        if s.targets.is_empty() {
            empty_icon(ui, Icon::Check, GOOD, t("No per-project target/ folders found."));
            return false;
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
                            ui.scope(|ui| {
                                ui.set_width(70.0);
                                icon_label(ui, Icon::Sparkle, 12.0, c, RichText::new(format!("{:.0}%", p * 100.0)).color(c));
                            })
                            .response
                            .on_hover_text(t("AI: chance you'll build this project again in the next 14 days"));
                        }
                        _ => {
                            ui.add_space(78.0);
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(!row.keep && self.busy.is_none(), ghost_button(t("Delete"))).clicked() {
                            let freed = remove_known(&row.path, row.size, false);
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
        changed
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
                if ui.add(ghost_button(t("+  Add folder…"))).clicked()
                    && let Some(p) = rfd::FileDialog::new().pick_folder()
                    && !self.cfg.roots.contains(&p)
                {
                    self.cfg.roots.push(p);
                }
                if ui.add_enabled(self.busy.is_none(), ghost_button(t("Search this computer"))).clicked() {
                    self.search_computer(ctx);
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
                        self.wizard = Some(Wizard { step: 0, integrate: !crate::integrate::is_integrated(), searched: false });
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
        // Held one frame past its life so the fade-out has something to animate.
        let showing = self.toast.as_ref().is_some_and(|(_, at)| at.elapsed() < Duration::from_secs(4));
        let alpha = fade(ctx, "toast", showing);
        if alpha <= 0.001 {
            self.toast = None;
            return;
        }
        let Some((text, _)) = &self.toast else { return };
        ctx.request_repaint();

        // Slides down into place as it fades in, and back up as it leaves.
        let drop = 18.0 - 10.0 * (1.0 - alpha);
        let tint = |c: Color32| c.gamma_multiply(alpha);
        egui::Area::new(egui::Id::unique("toast"))
            .anchor(egui::Align2::CENTER_TOP, vec2(0.0, drop))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(tint(CARD_HI))
                    .stroke(Stroke::new(1.0_f32, tint(LINE)))
                    .corner_radius(12)
                    .inner_margin(Margin::symmetric(18, 12))
                    .show(ui, |ui| {
                        ui.label(RichText::new(text.as_str()).color(tint(TEXT)).size(14.5));
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
                // Absolutised, not canonicalised: the user should see back the path
                // they typed, not Windows' `\\?\` form of it.
                let c = crate::util::tidy_abs(&p);
                if c.is_dir() {
                    if !self.cfg.roots.contains(&c) {
                        self.cfg.roots.push(c);
                    }
                    self.add_path.clear();
                } else {
                    self.toast(t("That folder doesn't exist"));
                }
            }
        });
    }
}

// ---------------- setup wizard ----------------

impl App {
    fn wizard_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
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
                        1 => self.w_folders(ui, ctx),
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
            (Icon::Package, t("One shared target folder"), t("Every project builds into the same place — no more target/ in each repo, and shared dependencies compile once.")),
            (Icon::Bolt, t("Lean builds"), t("Dependencies skip debug info, which is often half the size. Your own code keeps it.")),
            (Icon::Sparkle, t("Smart, and it cleans itself"), t("A tiny on-device model learns what you reuse and when you work, then cleans quietly every day.")),
        ] {
            ui.horizontal(|ui| {
                icon_ui(ui, icon, 22.0, if icon == Icon::Sparkle { AI } else { TEXT });
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).strong().color(TEXT));
                    ui.label(RichText::new(sub).size(13.0).color(MUTED));
                });
            });
            ui.add_space(8.0);
        }
    }

    fn w_folders(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // Nothing to suggest from the usual places: go and actually look, once.
        if self.cfg.roots.is_empty()
            && self.busy.is_none()
            && let Some(w) = self.wizard.as_mut()
            && !w.searched
        {
            w.searched = true;
            self.search_computer(ctx);
        }
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
            if ui.add(ghost_button(t("+  Choose folder…"))).clicked()
                && let Some(p) = rfd::FileDialog::new().pick_folder()
            {
                let p = crate::util::tidy_abs(&p);
                if !self.cfg.roots.contains(&p) {
                    self.cfg.roots.push(p);
                }
            }
            if ui
                .add_enabled(self.busy.is_none(), ghost_button(t("Search this computer")))
                .on_hover_text(t("Look through every drive for folders that contain Cargo projects"))
                .clicked()
            {
                self.search_computer(ctx);
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
            icon_label(
                ui,
                Icon::Warn,
                15.0,
                WARN,
                RichText::new(t("Move rtidy to your Applications folder first, then open it from there — otherwise the daily clean-up can't find it."))
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
        if crate::integrate::supported()
            && let Some(w) = self.wizard.as_mut()
        {
            let label = if cfg!(windows) { t("Add to Start menu") } else { t("Add to app menu") };
            option_row(ui, &mut w.integrate, label, t("So you can open rtidy like any other app"));
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
                    icon_label(ui, Icon::Check, 13.0, GOOD, RichText::new(&st).color(TEXT));
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
                    icon_ui(ui, Icon::Check, 54.0, GOOD);
                    ui.label(RichText::new(t("You're all set")).size(28.0).strong().color(TEXT));
                } else {
                    icon_ui(ui, Icon::Warn, 54.0, WARN);
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
                    3 => (t("Tidy up now"), !busy && self.snap.is_some()),
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
                            if self.wizard.as_ref().is_some_and(|w| w.integrate)
                                && let Err(e) = crate::integrate::integrate()
                            {
                                self.toast(tf("Couldn't add to menu: {}", &[&e.to_string()]));
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
    ctx.global_style_mut(|s| {
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
            // inside right-aligned headers egui lays out right-to-left: keep EN | ไทย order
            let mut items = [(Lang::En, "EN"), (Lang::Th, "ไทย")];
            if ui.layout().prefer_right_to_left() {
                items.reverse();
            }
            for (l, name) in items {
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

/// An empty state with a painted icon above the message.
fn empty_icon(ui: &mut egui::Ui, icon: Icon, color: Color32, text: &str) {
    card(CARD).inner_margin(Margin::same(28)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            icon_ui(ui, icon, 30.0, color);
            ui.add_space(8.0);
            ui.label(RichText::new(text).size(15.0).color(MUTED));
        });
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

// ---------------- icons ----------------
//
// Every symbol rtidy shows is drawn here rather than typed as a character.
//
// egui 0.37 moved its emoji fonts behind the optional `monochrome_emoji_fonts`
// feature, so glyphs like ✨ and 📦 fell back to tofu boxes; relying on the host's
// emoji font instead would render differently on Windows, macOS and Linux (and not
// at all on a bare Linux container). Painted shapes have no font dependency: they
// look the same everywhere, stay crisp at any DPI, and take the colour we ask for.

/// The symbols the UI needs. Each is drawn inside a unit square and scaled to fit.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// Smart/AI marker (was ✨)
    Sparkle,
    /// A shipping box: the shared target folder (was 📦)
    Package,
    /// Lightning: lean builds (was ⚡)
    Bolt,
    /// Success tick (was ✔ and 🎉)
    Check,
    /// Caution triangle (was ⚠)
    Warn,
    /// Circular arrow: rescan (was ⟳)
    Refresh,
}

/// Convex quad through four unit-square points.
fn quad(r: egui::Rect, pts: [(f32, f32); 4], color: Color32) -> egui::Shape {
    let p = pts.iter().map(|&(x, y)| r.lerp_inside(vec2(x, y))).collect();
    egui::Shape::convex_polygon(p, color, Stroke::NONE)
}

/// A four-pointed star, built from two convex diamonds so the tessellator never
/// has to fill a concave path.
fn star4(r: egui::Rect, c: (f32, f32), half: f32, color: Color32, out: &mut Vec<egui::Shape>) {
    let (cx, cy) = c;
    let w = half * 0.34; // waist: how fat the arms are
    out.push(quad(r, [(cx, cy - half), (cx + w, cy), (cx, cy + half), (cx - w, cy)], color));
    out.push(quad(r, [(cx - half, cy), (cx, cy - w), (cx + half, cy), (cx, cy + w)], color));
}

/// Draw `icon` to fill `rect` (a square is assumed; the shapes are centred in it).
pub fn paint_icon(painter: &egui::Painter, rect: egui::Rect, icon: Icon, color: Color32) {
    let r = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(rect.width().min(rect.height())));
    let w = r.width();
    let at = |x: f32, y: f32| r.lerp_inside(vec2(x, y));
    let mut shapes: Vec<egui::Shape> = Vec::new();

    match icon {
        Icon::Sparkle => {
            // One large star with two smaller companions, like the emoji it replaces.
            star4(r, (0.40, 0.42), 0.38, color, &mut shapes);
            star4(r, (0.78, 0.24), 0.17, color, &mut shapes);
            star4(r, (0.74, 0.74), 0.22, color, &mut shapes);
        }
        Icon::Package => {
            let s = Stroke::new(w * 0.09, color);
            // Box outline, lid seam and tape down the front.
            shapes.push(egui::Shape::rect_stroke(
                egui::Rect::from_min_max(at(0.10, 0.22), at(0.90, 0.88)),
                w * 0.08,
                s,
                egui::StrokeKind::Middle,
            ));
            shapes.push(egui::Shape::line_segment([at(0.10, 0.44), at(0.90, 0.44)], s));
            shapes.push(egui::Shape::line_segment([at(0.50, 0.22), at(0.50, 0.44)], s));
        }
        Icon::Bolt => {
            // Two triangles that overlap into a lightning bolt, each one convex.
            shapes.push(quad(r, [(0.62, 0.06), (0.20, 0.58), (0.52, 0.58), (0.52, 0.58)], color));
            shapes.push(quad(r, [(0.40, 0.94), (0.82, 0.42), (0.50, 0.42), (0.50, 0.42)], color));
        }
        Icon::Check => {
            let s = Stroke::new(w * 0.14, color);
            shapes.push(egui::Shape::line(vec![at(0.16, 0.54), at(0.40, 0.78), at(0.86, 0.24)], s));
        }
        Icon::Warn => {
            let s = Stroke::new(w * 0.10, color);
            shapes.push(egui::Shape::closed_line(
                vec![at(0.50, 0.10), at(0.94, 0.86), at(0.06, 0.86)],
                s,
            ));
            shapes.push(egui::Shape::line_segment([at(0.50, 0.38), at(0.50, 0.62)], s));
            shapes.push(egui::Shape::circle_filled(at(0.50, 0.75), w * 0.055, color));
        }
        Icon::Refresh => {
            // Open circle with an arrow head on the end, swept clockwise.
            let s = Stroke::new(w * 0.11, color);
            let (cx, cy, rad) = (0.5, 0.52, 0.36);
            let arc: Vec<egui::Pos2> = (0..=28)
                .map(|i| {
                    let t = std::f32::consts::TAU * (0.08 + 0.80 * i as f32 / 28.0);
                    at(cx + rad * t.cos(), cy + rad * t.sin())
                })
                .collect();
            shapes.push(egui::Shape::line(arc, s));
            let head = std::f32::consts::TAU * 0.08;
            let (hx, hy) = (cx + rad * head.cos(), cy + rad * head.sin());
            let a = w * 0.16 / w; // arrow half-size in unit space
            shapes.push(quad(
                r,
                [(hx + a * 0.2, hy - a * 1.5), (hx + a * 1.4, hy + a * 0.4), (hx - a * 0.6, hy + a * 0.7), (hx - a * 0.6, hy + a * 0.7)],
                color,
            ));
        }
    }
    painter.extend(shapes);
}

/// Allocate a square of `size` in the current layout and draw `icon` in it.
pub fn icon_ui(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_icon(ui.painter(), rect, icon, color);
    }
    resp
}

/// An icon followed by a text label, laid out on one line.
/// Replaces the old `"✨  some text"` string literals, so no translated string
/// has to carry a symbol in it.
fn icon_label(ui: &mut egui::Ui, icon: Icon, size: f32, icon_color: Color32, text: RichText) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = size * 0.42;
        icon_ui(ui, icon, size, icon_color);
        ui.label(text);
    });
}

/// A chip with a painted icon before its label.
fn chip_icon(ui: &mut egui::Ui, icon: Icon, text: &str, color: Color32) {
    Frame::new()
        .fill(color.linear_multiply(0.18))
        .corner_radius(8)
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                icon_ui(ui, icon, 11.0, color);
                ui.label(RichText::new(text).size(11.5).color(color));
            });
        });
}

/// A clickable button with a painted icon before its label.
///
/// Hand-drawn because egui's `Button` takes text or a texture, and these icons are
/// vector shapes with no texture behind them. Kept visually in step with `style()`:
/// same corner radius, same hover lift.
#[allow(clippy::too_many_arguments)]
fn icon_button(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: Icon,
    text: &str,
    text_size: f32,
    fill: Color32,
    stroke: Color32,
    fg: Color32,
    min: egui::Vec2,
) -> egui::Response {
    let icon_size = text_size * 1.05;
    let gap = text_size * 0.5;
    let pad = vec2(16.0, 9.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(text_size), fg);
    let content = vec2(icon_size + gap + galley.size().x, galley.size().y.max(icon_size));
    let size = (content + pad + pad).max(min);

    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    if ui.is_rect_visible(rect) {
        let hovered = enabled && resp.hovered();
        let bg = if hovered { fill.linear_multiply(1.18) } else { fill };
        ui.painter().rect(rect, CornerRadius::same(12), bg, Stroke::new(1.0_f32, stroke), egui::StrokeKind::Inside);
        let left = rect.center().x - content.x * 0.5;
        let icon_rect =
            egui::Rect::from_min_size(egui::pos2(left, rect.center().y - icon_size * 0.5), egui::Vec2::splat(icon_size));
        paint_icon(ui.painter(), icon_rect, icon, fg);
        let text_pos = egui::pos2(left + icon_size + gap, rect.center().y - galley.size().y * 0.5);
        ui.painter().galley(text_pos, galley, fg);
    }
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

// ---------------- animation ----------------
//
// egui redraws only when something changes, so motion has to be asked for. Each
// helper below drives a value through `Context::animate_*`, which interpolates
// towards the target and schedules the next repaint until it arrives.

/// Seconds for a value to travel to its new number.
const COUNT_SECS: f32 = 0.55;
/// Seconds for something to fade in or out.
const FADE_SECS: f32 = 0.25;

/// A byte count that glides to its new value instead of snapping.
///
/// Sizes change by gigabytes after a clean-up; watching the figure run down reads as
/// "this happened", where a jump just looks like a glitch.
fn anim_bytes(ctx: &egui::Context, key: &'static str, target: u64) -> u64 {
    // f32 loses precision above ~16 MB, so animate in megabytes and scale back.
    const UNIT: f64 = 1e6;
    let scaled = (target as f64 / UNIT) as f32;
    let now = ctx.animate_value_with_time(egui::Id::unique(("rtidy-anim", key)), scaled, COUNT_SECS);
    (f64::from(now.max(0.0)) * UNIT) as u64
}

/// A plain counter that glides to its new value.
fn anim_count(ctx: &egui::Context, key: &'static str, target: u64) -> u64 {
    let now = ctx.animate_value_with_time(egui::Id::unique(("rtidy-anim", key)), target as f32, COUNT_SECS);
    now.max(0.0).round() as u64
}

/// 0.0 while hidden, 1.0 while shown, easing between the two.
fn fade(ctx: &egui::Context, key: &'static str, shown: bool) -> f32 {
    ctx.animate_bool_with_time(egui::Id::unique(("rtidy-fade", key)), shown, FADE_SECS)
}

/// An indeterminate progress bar: a lozenge sweeping left to right.
///
/// Shown while a scan or a clean-up is running, so a long job looks like it is
/// moving rather than frozen. Driven by egui's clock, so it keeps the frame rate
/// steady instead of spinning the CPU.
fn sweep_bar(ui: &mut egui::Ui, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 3.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, LINE);

    // One pass every 1.4 s. `request_repaint` keeps it animating while visible.
    const PERIOD: f64 = 1.4;
    let phase = (ui.input(|i| i.time) % PERIOD) / PERIOD;
    let span = rect.width() * 0.34;
    // Ease in and out so the lozenge slows at each end rather than wrapping harshly.
    let eased = 0.5 - 0.5 * (phase as f32 * std::f32::consts::TAU).cos();
    let x = rect.left() - span + eased * (rect.width() + span);
    let lozenge = egui::Rect::from_min_size(
        egui::pos2(x.max(rect.left()), rect.top()),
        vec2(span.min(rect.right() - x.max(rect.left())).max(0.0), rect.height()),
    );
    painter.rect_filled(lozenge, 2.0, ACCENT);
    ui.ctx().request_repaint();
}
