//! Root UI: top bar with tabs, status bar, log panel and modal dialogs.

pub mod cleaner;
pub mod dashboard;
pub mod relocate;
pub mod theme;
pub mod widgets;

use std::path::Path;

use eframe::egui;

use theme::*;
use widgets as w;

use crate::app::{ConfirmRestore, LinkVaultApp, Tab};
use crate::util;
use crate::winapi;

pub fn root(app: &mut LinkVaultApp, ui: &mut egui::Ui) {
    top_bar(app, ui);
    status_bar(app, ui);
    if app.show_log {
        log_panel(app, ui);
    }

    egui::CentralPanel::no_frame().show(ui, |ui| {
        egui::Frame::new()
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| match app.tab {
                Tab::Dashboard => dashboard::ui(app, ui),
                Tab::Relocate => relocate::ui(app, ui),
                Tab::Cleaner => cleaner::ui(app, ui),
            });
    });

    let ctx = ui.ctx().clone();
    confirm_move_dialog(app, &ctx);
    confirm_restore_dialog(app, &ctx);
}

// --------------------------------------------------------------- top bar --

fn top_bar(app: &mut LinkVaultApp, ui: &mut egui::Ui) {
    egui::Panel::top("linkvault_top")
        .frame(
            egui::Frame::new()
                .fill(PANEL)
                .inner_margin(egui::Margin::symmetric(14, 9))
                .stroke(egui::Stroke::new(1.0, BORDER)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("LinkVault")
                        .size(19.0)
                        .strong()
                        .color(ACCENT),
                );
                ui.label(
                    egui::RichText::new(format!("v{}", crate::app::APP_VERSION))
                        .size(10.5)
                        .color(MUTED),
                );
                ui.add_space(14.0);

                for tab in [Tab::Dashboard, Tab::Relocate, Tab::Cleaner] {
                    let selected = app.tab == tab;
                    let text = egui::RichText::new(tab.label())
                        .size(13.5)
                        .color(if selected { egui::Color32::WHITE } else { MUTED });
                    let button = egui::Button::new(text)
                        .fill(if selected { ACCENT_DIM } else { egui::Color32::TRANSPARENT })
                        .stroke(egui::Stroke::new(
                            1.0,
                            if selected { ACCENT } else { egui::Color32::TRANSPARENT },
                        ))
                        .corner_radius(egui::CornerRadius::same(7))
                        .min_size(egui::vec2(0.0, 26.0));
                    if ui.add(button).clicked() {
                        app.tab = tab;
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if app.busy() {
                        ui.spinner();
                        if ui.add(w::ghost_button("Zrušit")).clicked() {
                            app.cancel_task();
                        }
                    }
                    if app.is_admin {
                        w::chip(ui, "ADMIN", GOOD);
                    } else {
                        w::chip(ui, "bez admin práv", WARN);
                    }
                    let target = util::ellipsize_middle(&app.config.target_root, 34);
                    ui.label(
                        egui::RichText::new(target)
                            .size(11.0)
                            .monospace()
                            .color(MUTED),
                    )
                    .on_hover_text(&app.config.target_root);
                });
            });
        });
}

// ------------------------------------------------------------ status bar --

fn status_bar(app: &mut LinkVaultApp, ui: &mut egui::Ui) {
    egui::Panel::bottom("linkvault_status")
        .frame(
            egui::Frame::new()
                .fill(PANEL)
                .inner_margin(egui::Margin::symmetric(14, 6))
                .stroke(egui::Stroke::new(1.0, BORDER)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(task) = &app.task {
                    let frac = task.progress.fraction();
                    let total = task.progress.total;
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .desired_width(180.0)
                            .desired_height(12.0)
                            .fill(ACCENT)
                            .corner_radius(egui::CornerRadius::same(6))
                            .animate(total == 0),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{}: {} · {} s",
                            task.kind.verb(),
                            task.progress.phase,
                            task.started.elapsed().as_secs()
                        ))
                        .size(11.5)
                        .color(TEXT),
                    );
                    if !task.progress.detail.is_empty() {
                        ui.label(
                            egui::RichText::new(util::ellipsize_middle(&task.progress.detail, 60))
                                .size(11.0)
                                .color(MUTED),
                        );
                    }
                } else {
                    ui.label(egui::RichText::new(&app.status).size(11.5).color(MUTED));
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = format!("Log ({})", app.log.len());
                    if ui.add(w::ghost_button(&label)).clicked() {
                        app.show_log = !app.show_log;
                    }
                    if let Some((text, kind, _)) = app.toast.clone() {
                        w::chip(ui, &text, w::log_color(kind));
                    }
                });
            });
        });
}

// ------------------------------------------------------------- log panel --

fn log_panel(app: &mut LinkVaultApp, ui: &mut egui::Ui) {
    egui::Panel::bottom("linkvault_log")
        .resizable(true)
        .default_size(150.0)
        .max_size(420.0)
        .frame(
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(11, 15, 20))
                .inner_margin(egui::Margin::same(10))
                .stroke(egui::Stroke::new(1.0, BORDER)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Průběh a chyby")
                        .size(12.0)
                        .strong()
                        .color(TEXT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(w::ghost_button("Zavřít")).clicked() {
                        app.show_log = false;
                    }
                    if ui.add(w::ghost_button("Vymazat")).clicked() {
                        app.log.clear();
                    }
                });
            });
            ui.add_space(2.0);
            egui::ScrollArea::vertical()
                .id_salt("log_scroll")
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &app.log {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(&line.stamp)
                                    .size(10.0)
                                    .monospace()
                                    .color(MUTED),
                            );
                            ui.label(
                                egui::RichText::new(w::log_icon(line.kind))
                                    .size(10.5)
                                    .color(w::log_color(line.kind)),
                            );
                            ui.label(
                                egui::RichText::new(&line.text)
                                    .size(11.0)
                                    .monospace()
                                    .color(w::log_color(line.kind)),
                            );
                        });
                    }
                });
        });
}

// ---------------------------------------------------------------- dialogs --

fn confirm_move_dialog(app: &mut LinkVaultApp, ctx: &egui::Context) {
    let Some(job) = app.confirm_move.clone() else {
        return;
    };

    let same_volume = util::same_volume(&job.entry.path, &job.target);
    let target_exists = job.target.exists();
    let volume = util::volume_root(&job.target).unwrap_or_else(|| job.target.clone());
    let free = winapi::available_space(&volume);
    let needed = job.entry.size + crate::relocator::FREE_SPACE_MARGIN;
    let enough_space = free >= needed;
    let can_start = !same_volume && !target_exists && enough_space && !app.busy();

    let mut start = false;
    let mut cancel = false;

    let response = egui::Modal::new(egui::Id::unique("confirm_move_modal")).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.label(
            egui::RichText::new("Přesunout složku na jiný disk?")
                .size(16.0)
                .strong()
                .color(TEXT),
        );
        ui.add_space(6.0);

        w::kv(ui, "Zdroj:", &job.entry.path.display().to_string());
        w::kv(ui, "Cíl:", &job.target.display().to_string());
        w::kv(
            ui,
            "Velikost:",
            &format!(
                "{} ({} souborů)",
                util::human_size(job.entry.size),
                job.entry.files
            ),
        );
        w::kv(
            ui,
            "Volné místo na cíli:",
            &format!("{} z {}", util::human_size(free), util::human_size(needed)),
        );

        ui.add_space(8.0);
        let mut problems: Vec<String> = Vec::new();
        if same_volume {
            problems.push(
                "Zdroj i cíl jsou na stejném disku — přesun nic neuvolní.".to_string(),
            );
        }
        if target_exists {
            problems.push(format!(
                "Cílový adresář už existuje: {}",
                job.target.display()
            ));
        }
        if !enough_space {
            problems.push(format!(
                "Na cílovém disku chybí místo (potřeba {} včetně rezervy 256 MiB).",
                util::human_size(needed)
            ));
        }
        for problem in &problems {
            ui.label(
                egui::RichText::new(format!("✖ {problem}"))
                    .size(11.5)
                    .color(ERROR),
            );
        }
        if problems.is_empty() {
            ui.label(
                egui::RichText::new(
                    "✔ Kontroly prošly: jiný svazek, cíl je volný, dostatek místa.",
                )
                .size(11.5)
                .color(GOOD),
            );
        }
        if !app.is_admin {
            ui.label(
                egui::RichText::new("! Běžíte bez admin práv — vytvoření odkazu může selhat.")
                    .size(11.5)
                    .color(WARN),
            );
        }

        ui.add_space(6.0);
        ui.checkbox(
            &mut app.config.keep_backup,
            egui::RichText::new("Ponechat .bak zálohu i po úspěšném přesunu").size(12.0),
        );
        ui.add_space(2.0);
        w::hint(
            ui,
            "Data se nejdřív zkopírují, ověří, a teprve pak se originál přejmenuje na .bak \
             a nahradí junction odkazem. Při jakékoli chybě dojde k rollbacku.",
        );

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(can_start, w::primary_button("Přesunout"))
                .clicked()
            {
                start = true;
            }
            if ui.add(w::ghost_button("Zrušit")).clicked() {
                cancel = true;
            }
        });
    });

    if start {
        app.confirm_move = None;
        app.start_move(ctx, job);
    } else if cancel || response.should_close() {
        app.confirm_move = None;
    }
}

fn confirm_restore_dialog(app: &mut LinkVaultApp, ctx: &egui::Context) {
    let Some(job) = app.confirm_restore.clone() else {
        return;
    };

    let source = job.entry.source_path();
    let target = job.entry.target_path();
    let linked = winapi::junction_target(&source)
        .map(|t| util::paths_eq(&t, &target))
        .unwrap_or(false);
    let free = winapi::available_space(
        &util::volume_root(&source).unwrap_or_else(|| Path::new("C:\\").to_path_buf()),
    );
    let needed = job.entry.bytes + crate::relocator::FREE_SPACE_MARGIN;
    let enough_space = free >= needed;
    let can_start = !app.busy() && enough_space;

    let mut delete_target = job.delete_target;
    let mut start = false;
    let mut cancel = false;

    let response = egui::Modal::new(egui::Id::unique("confirm_restore_modal")).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.label(
            egui::RichText::new("Vrátit složku zpět na disk C:?")
                .size(16.0)
                .strong()
                .color(TEXT),
        );
        ui.add_space(6.0);

        w::kv(ui, "Název:", &job.entry.name);
        w::kv(ui, "Data na:", &target.display().to_string());
        w::kv(ui, "Obnovit do:", &source.display().to_string());
        w::kv(ui, "Velikost:", &util::human_size(job.entry.bytes));
        w::kv(
            ui,
            "Volné místo na C:",
            &format!("{} z {}", util::human_size(free), util::human_size(needed)),
        );

        ui.add_space(8.0);
        if !linked {
            ui.label(
                egui::RichText::new("! Odkaz neodpovídá záznamu v config.toml.")
                    .size(11.5)
                    .color(WARN),
            );
        }
        if !enough_space {
            ui.label(
                egui::RichText::new("✖ Na systémovém disku není dost místa.").size(11.5).color(ERROR),
            );
        } else {
            ui.label(
                egui::RichText::new("✔ Data se zkopírují na C:, odkaz se odstraní a složka se obnoví.")
                    .size(11.5)
                    .color(GOOD),
            );
        }

        ui.add_space(6.0);
        ui.checkbox(
            &mut delete_target,
            egui::RichText::new("Smazat i zkopírovaná data na cílovém disku").size(12.0),
        );
        ui.add_space(2.0);
        w::hint(
            ui,
            "Bez zaškrtnutí zůstanou data na cílovém disku jako záloha — můžete je smazat ručně.",
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(can_start, w::primary_button("Vrátit na C:"))
                .clicked()
            {
                start = true;
            }
            if ui.add(w::ghost_button("Zrušit")).clicked() {
                cancel = true;
            }
        });
    });

    if start {
        app.confirm_restore = None;
        app.start_restore(
            ctx,
            ConfirmRestore {
                entry: job.entry,
                delete_target,
            },
        );
    } else if cancel || response.should_close() {
        app.confirm_restore = None;
    }
}