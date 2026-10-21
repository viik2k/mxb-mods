//! The window: archives in Downloads with an Install button each, drag-and-drop, installed list.
//! Sized for the Steam Deck's touchscreen too.

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::SystemTime;
use std::{env, fs};

use eframe::egui;

type Job = JoinHandle<Result<Vec<String>, String>>;

struct State {
    mods: PathBuf,
    downloads: Vec<(PathBuf, u64)>,
    installed: Vec<String>,
    status: String,
    job: Option<Job>,
}

pub fn run(mods: PathBuf) {
    #[cfg(windows)]
    hide_own_console();
    let mut s = State { mods, downloads: vec![], installed: vec![], status: String::new(), job: None };
    s.refresh();
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MX Bikes mods")
            .with_inner_size([900.0, 640.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    let mut styled = false;
    let result = eframe::run_ui_native("mxb-mods", opts, move |ui, _| {
        if !styled {
            styled = true;
            ui.ctx().set_zoom_factor(1.25);
            ui.ctx().all_styles_mut(|st| {
                st.spacing.button_padding = egui::vec2(14.0, 8.0);
                st.spacing.item_spacing = egui::vec2(10.0, 8.0);
            });
        }
        egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
    });
    if let Err(e) = result {
        eprintln!("can't open the window: {e}");
        std::process::exit(1);
    }
}

impl State {
    fn refresh(&mut self) {
        self.downloads = downloads();
        self.installed = crate::list(&self.mods).unwrap_or_default();
    }

    fn start(&mut self, src: PathBuf, ctx: &egui::Context) {
        if self.job.is_some() {
            return;
        }
        self.status = format!("installing {}…", name(&src));
        let (mods, ctx) = (self.mods.clone(), ctx.clone());
        self.job = Some(std::thread::spawn(move || {
            let r = crate::install(&src, &mods).map_err(|e| e.to_string());
            ctx.request_repaint();
            r
        }));
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.job.as_ref().is_some_and(|j| j.is_finished()) {
            self.status = match self.job.take().unwrap().join() {
                Ok(Ok(lines)) => lines.join("\n"),
                Ok(Err(e)) => format!("error: {e}"),
                Err(_) => "error: install crashed".into(),
            };
            self.refresh();
        }
        let busy = self.job.is_some();
        for f in ctx.input(|i| i.raw.dropped_files.clone()) {
            self.start(f.path().to_path_buf(), &ctx);
        }

        ui.heading("MX Bikes mods");
        if self.mods.is_dir() {
            ui.label(format!("Mods folder: {}", self.mods.display()));
        } else {
            ui.colored_label(ui.visuals().error_fg_color, format!("Mods folder not found: {}", self.mods.display()));
        }
        ui.horizontal(|ui| {
            if busy {
                ui.spinner();
            }
            let col = if self.status.starts_with("error") { ui.visuals().error_fg_color } else { ui.visuals().text_color() };
            ui.colored_label(col, if self.status.is_empty() { "Drop a mod archive or folder here, or pick one below." } else { &self.status });
        });
        ui.separator();

        ui.horizontal(|ui| {
            ui.strong("Downloads");
            if ui.button("Refresh").clicked() {
                self.refresh();
            }
        });
        let mut pick = None;
        egui::ScrollArea::vertical().id_salt("downloads").max_height(ui.available_height() * 0.5).show(ui, |ui| {
            if self.downloads.is_empty() {
                ui.weak("No .zip / .rar / .7z / .pkz files in Downloads.");
            }
            for (path, size) in &self.downloads {
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new("Install")).clicked() {
                        pick = Some(path.clone());
                    }
                    ui.label(format!("{}  ({:.1} MB)", name(path), *size as f64 / 1e6));
                });
            }
        });
        if let Some(p) = pick {
            self.start(p, &ctx);
        }
        ui.separator();

        ui.strong("Installed");
        egui::ScrollArea::vertical().id_salt("installed").auto_shrink([false, false]).show(ui, |ui| {
            for l in &self.installed {
                ui.monospace(l);
            }
        });
    }
}

/// Mod archives in ~/Downloads, newest first.
fn downloads() -> Vec<(PathBuf, u64)> {
    let home = env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default();
    let Ok(rd) = fs::read_dir(Path::new(&home).join("Downloads")) else { return vec![] };
    let mut v: Vec<_> = rd
        .flatten()
        .filter(|e| ["zip", "rar", "7z", "pkz"].iter().any(|x| crate::has_ext(&e.path(), x)))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((e.path(), m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
        })
        .collect();
    v.sort_by(|a, b| b.2.cmp(&a.2));
    v.into_iter().map(|(p, n, _)| (p, n)).collect()
}

fn name(p: &Path) -> String {
    p.file_name().unwrap_or(p.as_os_str()).to_string_lossy().into_owned()
}

/// Double-clicked from Explorer, Windows gives us a console window of our own: drop it.
/// Started from a terminal, the console is shared, so keep it for error output.
#[cfg(windows)]
fn hide_own_console() {
    unsafe extern "system" {
        fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
        fn FreeConsole() -> i32;
    }
    let mut pids = [0u32; 2];
    unsafe {
        if GetConsoleProcessList(pids.as_mut_ptr(), 2) == 1 {
            FreeConsole();
        }
    }
}

