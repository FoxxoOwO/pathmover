//! Tab 1 — disk overview and quick actions.

use eframe::egui;

use super::theme::*;
use super::widgets;
use crate::app::PathMoverApp;
use crate::util;
use crate::winapi;

pub fn ui(app: &mut PathMoverApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(4.0);

            if !app.is_admin {
                admin_banner(app, ui);
                ui.add_space(8.0);
            }

            summary_row(app, ui);
            ui.add_space(10.0);

            widgets::section_title(
                ui,
                "Disky",
                Some("obsazení a volné místo (aktualizace každých 10 s)"),
            );
            let disks = app.disks.clone();
            for disk in &disks {
                disk_card(app, ui, disk);
                ui.add_space(6.0);
            }
            if disks.is_empty() {
                widgets::hint(ui, "Nepodařilo se načíst informace o discích.");
            }

            ui.add_space(10.0);
            widgets::section_title(ui, "Cílový adresář pro přesunutá data", None);
            target_settings(app, ui);

            ui.add_space(10.0);
            widgets::section_title(ui, "Rychlé akce", None);
            let ctx = ui.ctx().clone();
            quick_actions(app, ui, ctx);

            ui.add_space(16.0);
        });
}

fn admin_banner(app: &mut PathMoverApp, ui: &mut egui::Ui) {
    egui::Frame::new()
        .fill(WARN.gamma_multiply(0.12))
        .stroke(egui::Stroke::new(1.0, WARN.gamma_multiply(0.6)))
        .corner_radius(egui::CornerRadius::same(9))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("⚠").size(18.0).color(WARN));
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("Aplikace neběží jako správce")
                            .strong()
                            .color(WARN),
                    );
                    widgets::hint(
                        ui,
                        "Vytváření junction odkazů a práce s některými soubory v AppData vyžaduje \
                         zvýšená oprávnění. Skenování a čištění cache funguje i bez nich.",
                    );
                });
                ui.add_space(8.0);
                if ui
                    .add(widgets::ghost_button("Restartovat jako správce"))
                    .clicked()
                {
                    if winapi::relaunch_as_admin() {
                        app.log_push(
                            crate::app::LogKind::Good,
                            "Spouštím novou instanci se zvýšenými právy…",
                        );
                    } else {
                        app.log_push(
                            crate::app::LogKind::Error,
                            "Nepodařilo se spustit aplikaci jako správce (ShellExecuteW selhal).",
                        );
                    }
                }
            });
        });
}

fn summary_row(app: &mut PathMoverApp, ui: &mut egui::Ui) {
    let system = app.system_disk().cloned();
    let moved_total = app.config.total_moved_bytes();
    let cache_total: u64 = app.caches.iter().map(|c| c.size).sum();

    let mut tiles: Vec<(&str, String, egui::Color32)> = Vec::with_capacity(5);
    if let Some(disk) = &system {
        let used = disk.total.saturating_sub(disk.free);
        let frac = if disk.total > 0 {
            used as f32 / disk.total as f32
        } else {
            0.0
        };
        tiles.push((
            "Volné místo na C:",
            util::human_size(disk.free),
            if frac > 0.9 { ERROR } else { GOOD },
        ));
        tiles.push(("Celkem C:", util::human_size(disk.total), TEXT));
    }
    tiles.push((
        "Přesunuto na jiné disky",
        util::human_size(moved_total),
        ACCENT,
    ));
    tiles.push(("Nalezená cache", util::human_size(cache_total), WARN));
    tiles.push(("Složek v AppData", app.folders.len().to_string(), TEXT));
    widgets::stat_row(ui, &tiles);

    if let Some(disk) = &system {
        let used = disk.total.saturating_sub(disk.free);
        let frac = if disk.total > 0 {
            used as f32 / disk.total as f32
        } else {
            0.0
        };
        ui.add_space(8.0);
        widgets::usage_bar(
            ui,
            frac,
            if frac > 0.9 {
                ERROR
            } else if frac > 0.75 {
                WARN
            } else {
                ACCENT
            },
        );
        ui.add_space(2.0);
        widgets::hint(
            ui,
            format!(
                "C: je zaplněno z {:.0} % ({} z {})",
                frac * 100.0,
                util::human_size(used),
                util::human_size(disk.total)
            ),
        );
    }
}

fn disk_card(app: &mut PathMoverApp, ui: &mut egui::Ui, disk: &crate::app::DiskInfo) {
    let used = disk.total.saturating_sub(disk.free);
    let frac = if disk.total > 0 {
        used as f32 / disk.total as f32
    } else {
        0.0
    };

    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(&disk.name)
                    .size(15.0)
                    .strong()
                    .color(TEXT),
            );
            if disk.is_system {
                widgets::chip(ui, "SYSTÉMOVÝ", ACCENT);
            }
            if app.config.target_root.to_ascii_uppercase().starts_with(
                &disk.mount.to_ascii_uppercase(),
            ) {
                widgets::chip(ui, "CÍL PŘESUNU", GOOD);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{} volných z {}",
                        util::human_size(disk.free),
                        util::human_size(disk.total)
                    ))
                    .size(12.5)
                    .color(MUTED),
                );
            });
        });
        ui.add_space(4.0);
        widgets::usage_bar(
            ui,
            frac,
            if frac > 0.9 {
                ERROR
            } else if frac > 0.75 {
                WARN
            } else {
                ACCENT
            },
        );
        if !disk.is_system {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add(widgets::ghost_button("Použít jako cíl přesunu"))
                    .clicked()
                {
                    app.config.target_root =
                        format!("{}\\AppDataMoved", disk.mount.trim_end_matches('\\'));
                    app.target_root_edit = app.config.target_root.clone();
                    app.log_push(
                        crate::app::LogKind::Info,
                        format!("Cílový adresář nastaven na {}", app.config.target_root),
                    );
                    app.save_config();
                }
                if ui
                    .add(widgets::ghost_button("Otevřít v Průzkumníku"))
                    .clicked()
                {
                    if let Err(err) = winapi::open_in_explorer(std::path::Path::new(&disk.mount)) {
                        app.log_push(crate::app::LogKind::Error, err.to_string());
                    }
                }
            });
        }
    });
}

fn target_settings(app: &mut PathMoverApp, ui: &mut egui::Ui) {
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Cesta:").size(12.5).color(MUTED));
            // Leave room for the save button so the row spans the whole card.
            let edit_width = (ui.available_width() - 96.0).max(180.0);
            ui.add(
                egui::TextEdit::singleline(&mut app.target_root_edit)
                    .desired_width(edit_width)
                    .font(egui::TextStyle::Monospace),
            );
            let changed = app.target_root_edit != app.config.target_root;
            if ui
                .add_enabled(changed, widgets::primary_button("Uložit"))
                .clicked()
            {
                app.config.target_root = app.target_root_edit.clone();
                app.save_config();
            }
        });
        ui.add_space(4.0);
        widgets::hint(
            ui,
            "Složky se přesouvají do podadresářů Local\\<název> a Roaming\\<název> v této cestě. \
             Cíl musí ležet na jiném disku než C:, jinak přesun nemá smysl.",
        );
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(widgets::ghost_button("Doporučit největší disk"))
                .clicked()
            {
                let best = app
                    .disks
                    .iter()
                    .filter(|d| !d.is_system && d.free > 0)
                    .max_by_key(|d| d.free)
                    .cloned();
                match best {
                    Some(disk) => {
                        app.config.target_root =
                            format!("{}\\AppDataMoved", disk.mount.trim_end_matches('\\'));
                        app.target_root_edit = app.config.target_root.clone();
                        app.save_config();
                    }
                    None => app.log_push(
                        crate::app::LogKind::Warn,
                        "Nebyl nalezen žádný jiný pevný disk než C:",
                    ),
                }
            }
            if ui.add(widgets::ghost_button("Otevřít")).clicked() {
                if let Err(err) = winapi::open_in_explorer(std::path::Path::new(&app.config.target_root)) {
                    app.log_push(crate::app::LogKind::Error, err.to_string());
                }
            }
            ui.checkbox(
                &mut app.config.keep_backup,
                egui::RichText::new("Ponechat .bak zálohu po přesunu").size(12.5),
            );
        });
    });
}

fn quick_actions(app: &mut PathMoverApp, ui: &mut egui::Ui, ctx: egui::Context) {
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let busy = app.busy();
            if ui
                .add_enabled(!busy, widgets::primary_button("Skenovat AppData"))
                .clicked()
            {
                app.start_scan(&ctx);
            }
            if ui
                .add_enabled(!busy, widgets::ghost_button("Najít cache"))
                .clicked()
            {
                app.start_cache_scan(&ctx);
            }
            if ui
                .add_enabled(!busy, widgets::ghost_button("Obnovit disky"))
                .clicked()
            {
                app.refresh_disks();
            }
        });
        ui.add_space(6.0);
        widgets::hint(
            ui,
            "Sken projde AppData\\Local a AppData\\Roaming, změří velikost každé složky a zjistí, \
             které už jsou přesunuté přes junction.",
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            widgets::kv(ui, "Stav:", &app.status);
            widgets::kv(ui, "Uložené přesuny:", &app.config.entries.len().to_string());
            widgets::kv(
                ui,
                "Config:",
                &crate::config::Config::path().display().to_string(),
            );
        });
    });
}