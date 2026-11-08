//! Tab 3 — Electron cache finder and cleaner.

use eframe::egui;

use super::theme::*;
use super::widgets as w;
use crate::app::{LogKind, LinkVaultApp};
use crate::util;
use crate::winapi;

pub fn ui(app: &mut LinkVaultApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let busy = app.busy();

    w::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!busy, w::primary_button("Najít cache"))
                .clicked()
            {
                app.start_cache_scan(&ctx);
            }
            if ui
                .add_enabled(!busy, w::ghost_button("Vybrat vše"))
                .clicked()
            {
                for cache in &mut app.caches {
                    if !cache.is_link {
                        cache.selected = true;
                    }
                }
            }
            if ui
                .add_enabled(!busy, w::ghost_button("Odznačit vše"))
                .clicked()
            {
                for cache in &mut app.caches {
                    cache.selected = false;
                }
            }
            ui.separator();
            ui.checkbox(&mut app.clean_logs, "Zahrnout i logs");
            ui.separator();

            let count = app.selected_cache_count();
            let bytes = app.selected_cache_bytes();
            let label = format!("Vyčistit cache ({})", util::human_size(bytes));
            if ui
                .add_enabled(!busy && count > 0, w::danger_button(&label))
                .clicked()
            {
                app.start_clean(&ctx);
            }
            if busy && ui.add(w::ghost_button("Zrušit")).clicked() {
                app.cancel_task();
            }
        });
        ui.add_space(6.0);
        w::hint(
            ui,
            "Mažou se pouze soubory uvnitř těchto složek, ne složky samotné. Soubory zamčené \
             běžící aplikací se přeskočí a objeví se v přehledu chyb.",
        );
    });

    if let Some(task) = &app.task {
        ui.add_space(8.0);
        w::progress_row(ui, &task.progress);
    }

    ui.add_space(10.0);

    let total: u64 = app.caches.iter().map(|c| c.size).sum();
    let files: u64 = app.caches.iter().map(|c| c.files).sum();
    let tiles: Vec<(&str, String, egui::Color32)> = vec![
        ("Celkem odpadu", util::human_size(total), WARN),
        (
            "Vybráno",
            util::human_size(app.selected_cache_bytes()),
            ERROR,
        ),
        ("Cache složek", app.caches.len().to_string(), ON_SURFACE_VARIANT),
        ("Souborů", files.to_string(), MUTED),
    ];
    w::stat_row(ui, &tiles);

    if let Some(report) = app.clean_report.clone() {
        ui.add_space(10.0);
        report_card(ui, &report);
    }

    ui.add_space(10.0);

    if app.caches.is_empty() {
        w::empty_state(
            ui,
            "Zatím nebyly nalezeny žádné cache složky",
            "Spusťte „Najít cache“ — prohledá AppData\\Local a Roaming do hloubky 4 úrovní.",
        );
        return;
    }

    let mut pending_open: Option<String> = None;
    let caches = app.caches.clone();

    egui::ScrollArea::vertical()
        .id_salt("cache_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, cache) in caches.iter().enumerate() {
                w::card_hover(ui, |ui| {
                    ui.horizontal(|ui| {
                        let mut selected = cache.selected;
                        if ui
                            .add_enabled(!cache.is_link, egui::Checkbox::without_text(&mut selected))
                            .changed()
                        {
                            if let Some(slot) = app.caches.get_mut(index) {
                                slot.selected = selected;
                            }
                        }
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                w::chip(ui, &cache.app, ACCENT);
                                ui.label(
                                    egui::RichText::new(&cache.kind)
                                        .size(13.0)
                                        .strong()
                                        .color(ON_SURFACE),
                                );
                                w::chip(
                                    ui,
                                    &format!(
                                        "{} / {} souborů",
                                        util::human_size(cache.size),
                                        cache.files
                                    ),
                                    if cache.size > 100 * 1024 * 1024 {
                                        WARN
                                    } else {
                                        MUTED
                                    },
                                );
                                if cache.is_link {
                                    w::chip(ui, "junction", MUTED);
                                }
                            });
                            ui.label(
                                egui::RichText::new(util::ellipsize_middle(
                                    &cache.display_path(),
                                    100,
                                ))
                                .size(11.0)
                                .monospace()
                                .color(MUTED),
                            )
                            .on_hover_text(cache.display_path());
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(w::ghost_button("Otevřít")).clicked() {
                                pending_open = Some(cache.display_path());
                            }
                        });
                    });
                });
                ui.add_space(4.0);
            }
        });

    if let Some(path) = pending_open {
        if let Err(err) = winapi::open_in_explorer(std::path::Path::new(&path)) {
            app.log_push(LogKind::Error, err.to_string());
        }
    }
}

fn report_card(ui: &mut egui::Ui, report: &crate::cleaner::CleanReport) {
    let color = if report.success() { GOOD } else { WARN };
    w::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("Poslední čištění")
                    .size(14.0)
                    .strong()
                    .color(color),
            );
            w::chip(
                ui,
                &format!("uvolněno {}", util::human_size(report.freed)),
                GOOD,
            );
            w::chip(
                ui,
                &format!("{} souborů v {} složkách", report.deleted, report.dirs),
                MUTED,
            );
            if report.cancelled {
                w::chip(ui, "zrušeno", WARN);
            }
            if !report.errors.is_empty() {
                w::chip(ui, &format!("{} chyb", report.errors.len()), ERROR);
            }
        });
        if !report.errors.is_empty() {
            ui.add_space(6.0);
            egui::CollapsingHeader::new(
                egui::RichText::new("Podrobnosti chyb")
                    .size(12.5)
                    .color(MUTED),
            )
            .id_salt("clean_errors")
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("clean_errors_scroll")
                    .max_height(140.0)
                    .show(ui, |ui| {
                        for err in &report.errors {
                            ui.label(
                                egui::RichText::new(err)
                                    .size(11.0)
                                    .monospace()
                                    .color(ERROR),
                            );
                        }
                    });
            });
        }
    });
}