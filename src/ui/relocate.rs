//! Tab 2 — AppData folder list, move and rollback.

use std::path::PathBuf;

use eframe::egui;

use super::theme::*;
use super::widgets;
use crate::app::{ConfirmMove, ConfirmRestore, LinkVaultApp, SortMode};
use crate::config::MovedEntry;
use crate::scanner::{FolderEntry, FolderState, Scope};
use crate::util;
use crate::winapi;

const MAX_ROWS: usize = 400;

enum RowAction {
    Move(Box<FolderEntry>),
    Restore(Box<MovedEntry>),
    Open(PathBuf),
}

pub fn ui(app: &mut LinkVaultApp, ui: &mut egui::Ui) {
    toolbar(app, ui);
    ui.add_space(6.0);

    if let Some(task) = &app.task {
        widgets::progress_row(ui, &task.progress);
        ui.add_space(6.0);
    }

    let rows = app.visible_folders();
    let shown = rows.len().min(MAX_ROWS);

    if rows.is_empty() {
        if app.scanned_once {
            widgets::empty_state(
                ui,
                "Žádná složka neodpovídá filtru",
                "Zkuste zvýšit limit velikosti, zrušit filtr nebo spustit nový sken.",
            );
        } else {
            widgets::empty_state(
                ui,
                "Zatím nebylo skenováno",
                "Spusťte sken tlačítkem výše — projde AppData a změří velikost složek.",
            );
        }
        history_section(app, ui);
        return;
    }

    let local_bytes = app.local_bytes_total();
    ui.horizontal_wrapped(|ui| {
        widgets::chip(ui, &format!("{} složek", rows.len()), MUTED);
        widgets::chip(
            ui,
            &format!("na C: {}", util::human_size(local_bytes)),
            WARN,
        );
        widgets::chip(
            ui,
            &format!("přesunuto: {}", app.config.entries.len()),
            GOOD,
        );
        if shown < rows.len() {
            widgets::chip(
                ui,
                &format!("zobrazeno {shown} z {}", rows.len()),
                MUTED,
            );
        }
    });
    ui.add_space(6.0);

    let mut pending: Option<RowAction> = None;
    let ctx = ui.ctx().clone();

    table_header(ui);

    egui::ScrollArea::vertical()
        .id_salt("folders_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (row_index, index) in rows.iter().take(MAX_ROWS).enumerate() {
                let folder = app.folders[*index].clone();
                widgets::list_row(ui, row_index % 2 == 1, |ui| {
                    folder_row(ui, app, &folder, &mut pending);
                });
            }
        });

    if let Some(action) = pending {
        apply_action(app, ui, &ctx, action);
    }

    ui.add_space(10.0);
    history_section(app, ui);
}

fn toolbar(app: &mut LinkVaultApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!app.busy(), widgets::primary_button("Nový sken"))
                .clicked()
            {
                app.start_scan(&ctx);
            }
            ui.separator();
            ui.label(egui::RichText::new("Hledat:").size(12.0).color(MUTED));
            ui.add(
                egui::TextEdit::singleline(&mut app.search)
                    .hint_text("název složky")
                    .desired_width(160.0),
            );
            ui.label(egui::RichText::new("Min. velikost:").size(12.0).color(MUTED));
            ui.add(
                egui::Slider::new(&mut app.min_size_mb, 0.0..=5000.0)
                    .suffix(" MB")
                    .logarithmic(true),
            );
            egui::ComboBox::from_id_salt("sort_combo")
                .width(150.0)
                .selected_text(app.sort.label())
                .show_ui(ui, |ui| {
                    for mode in [SortMode::SizeDesc, SortMode::NameAsc, SortMode::StatusFirst] {
                        ui.selectable_value(&mut app.sort, mode, mode.label());
                    }
                });
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut app.scope_local, Scope::Local.label());
            ui.checkbox(&mut app.scope_roaming, Scope::Roaming.label());
            ui.checkbox(&mut app.only_moved, "Jen přesunuté");
            ui.separator();
            if ui
                .add(widgets::ghost_button("Otevřít AppData"))
                .clicked()
            {
                let root = std::env::var_os("USERPROFILE")
                    .map(|p| PathBuf::from(p).join("AppData"))
                    .unwrap_or_else(|| PathBuf::from("C:\\Users"));
                if let Err(err) = winapi::open_in_explorer(&root) {
                    app.log_push(crate::app::LogKind::Error, err.to_string());
                }
            }
        });
    });
}

/// Column widths of the folder table. The name column takes whatever is left,
/// so the table always spans the whole window.
const COL_SIZE: f32 = 86.0;
const COL_STATE: f32 = 190.0;
const COL_ACTIONS: f32 = 215.0;

/// Column widths of the folder table, derived from the width on offer so the
/// table always spans the window and never pushes the buttons off-screen.
#[derive(Clone, Copy)]
struct Columns {
    name: f32,
    size: f32,
    state: f32,
    actions: f32,
}

impl Columns {
    fn for_width(total: f32) -> Self {
        let actions = (total * 0.24).clamp(150.0, COL_ACTIONS);
        let state = (total * 0.20).clamp(120.0, COL_STATE);
        let size = COL_SIZE;
        // 36 px of padding leaves room for the item spacing between columns.
        let name = (total - actions - state - size - 36.0).max(170.0);
        Self {
            name,
            size,
            state,
            actions,
        }
    }
}

fn table_header(ui: &mut egui::Ui) {
    // Same horizontal padding as `list_row` so header and rows line up.
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(10, 0))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let cols = Columns::for_width(ui.available_width());
            ui.horizontal(|ui| {
                header_cell(ui, "Složka", cols.name);
                header_cell(ui, "Velikost", cols.size);
                header_cell(ui, "Stav", cols.state);
                header_cell(ui, "Akce", cols.actions);
            });
        });
    ui.add_space(2.0);
}

fn header_cell(ui: &mut egui::Ui, text: &str, width: f32) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 0.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(
                egui::RichText::new(text.to_uppercase())
                    .size(10.0)
                    .color(MUTED),
            );
        },
    );
}

/// Render one folder row. Columns mirror `table_header` so everything lines up.
fn folder_row(
    ui: &mut egui::Ui,
    app: &LinkVaultApp,
    folder: &FolderEntry,
    pending: &mut Option<RowAction>,
) {
    let cols = Columns::for_width(ui.available_width());

    ui.horizontal(|ui| {
        // Column 1: name, scope and path.
        ui.allocate_ui_with_layout(
            egui::vec2(cols.name, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_width(cols.name);
                ui.horizontal(|ui| {
                    let (scope_label, scope_color) = match folder.scope {
                        Scope::Local => ("Local", ACCENT),
                        Scope::Roaming => ("Roaming", egui::Color32::from_rgb(167, 139, 250)),
                    };
                    widgets::chip(ui, scope_label, scope_color);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&folder.name)
                                .size(13.5)
                                .strong()
                                .color(TEXT),
                        )
                        .truncate(),
                    );
                    if app.config.entry_for(&folder.path).is_some()
                        && matches!(folder.state, FolderState::Local)
                    {
                        widgets::chip(ui, "záznam bez odkazu", WARN);
                    }
                });
                let mut path_text = folder.path.display().to_string();
                if let FolderState::Junction { target, .. } = &folder.state {
                    path_text = format!("{} → {}", path_text, target.display());
                }
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&path_text)
                            .size(10.5)
                            .monospace()
                            .color(MUTED),
                    )
                    .truncate(),
                )
                .on_hover_text(path_text);
            },
        );

        // Column 2: size.
        ui.allocate_ui_with_layout(
            egui::vec2(cols.size, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_width(cols.size);
                ui.label(
                    egui::RichText::new(util::short_size(folder.size))
                        .size(13.0)
                        .monospace()
                        .color(TEXT),
                )
                .on_hover_text(util::human_size(folder.size));
                ui.label(
                    egui::RichText::new(format!("{} souborů", folder.files))
                        .size(10.5)
                        .color(MUTED),
                );
            },
        );

        // Column 3: state.
        let (state_text, state_color) = match &folder.state {
            FolderState::Local => ("Na disku C:".to_string(), WARN),
            FolderState::Junction { healthy, target } => {
                if *healthy {
                    (
                        format!("Přesunuto ({})", util::drive_of(target).unwrap_or_default()),
                        GOOD,
                    )
                } else {
                    ("Odkaz nefunkční".to_string(), ERROR)
                }
            }
            FolderState::Broken { reason } => (format!("Poškozeno: {reason}"), ERROR),
            FolderState::Missing => ("Složka nenalezena".to_string(), MUTED),
        };
        ui.allocate_ui_with_layout(
            egui::vec2(cols.state, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_width(cols.state);
                widgets::chip(ui, &state_text, state_color)
                    .on_hover_text(folder.state.short_label());
                if let FolderState::Junction { target, .. } = &folder.state {
                    let target_text = target.display().to_string();
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&target_text).size(10.0).color(MUTED),
                        )
                        .truncate(),
                    )
                    .on_hover_text(target_text);
                }
            },
        );

        // Column 4: actions, right-aligned inside their column.
        ui.allocate_ui_with_layout(
            egui::vec2(cols.actions, 0.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.set_min_width(cols.actions);
                let busy = app.busy();
                if ui.add(widgets::ghost_button("Otevřít")).clicked() {
                    *pending = Some(RowAction::Open(folder.path.clone()));
                }
                match &folder.state {
                    FolderState::Local | FolderState::Broken { .. } => {
                        let enabled = !busy && folder.can_move();
                        if ui
                            .add_enabled(enabled, widgets::primary_button("Přesunout…"))
                            .clicked()
                        {
                            *pending = Some(RowAction::Move(Box::new(folder.clone())));
                        }
                    }
                    FolderState::Junction { .. } => {
                        let enabled = !busy && folder.can_restore();
                        if ui
                            .add_enabled(enabled, widgets::ghost_button("Vrátit na C:…"))
                            .clicked()
                        {
                            let entry = folder
                                .managed
                                .clone()
                                .or_else(|| app.config.entry_for(&folder.path).cloned());
                            if let Some(entry) = entry {
                                *pending = Some(RowAction::Restore(Box::new(entry)));
                            }
                        }
                    }
                    FolderState::Missing => {
                        ui.add_enabled(false, widgets::ghost_button("—"));
                    }
                }
            },
        );
    });
}

fn apply_action(
    app: &mut LinkVaultApp,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    action: RowAction,
) {
    match action {
        RowAction::Move(folder) => {
            let target = app.config.target_for(folder.scope.dir_name(), &folder.name);
            if !app.is_admin {
                app.log_push(
                    crate::app::LogKind::Warn,
                    "Přesun bez admin práv obvykle selže na vytvoření junction.",
                );
            }
            app.confirm_move = Some(ConfirmMove {
                entry: *folder,
                target,
                keep_backup: app.config.keep_backup,
            });
        }
        RowAction::Restore(entry) => {
            app.confirm_restore = Some(ConfirmRestore {
                entry: *entry,
                delete_target: false,
            });
        }
        RowAction::Open(path) => {
            if let Err(err) = winapi::open_in_explorer(&path) {
                app.log_push(crate::app::LogKind::Error, err.to_string());
            }
        }
    }
    let _ = (ui, ctx);
}

/// Recorded moves from config.toml (also serves as a rollback list).
fn history_section(app: &mut LinkVaultApp, ui: &mut egui::Ui) {
    if app.config.entries.is_empty() {
        return;
    }
    ui.add_space(6.0);
    widgets::section_title(
        ui,
        "Historie přesunů",
        Some("(uloženo v config.toml)"),
    );

    let mut pending: Option<RowAction> = None;
    let entries = app.config.entries.clone();
    let busy = app.busy();

    egui::ScrollArea::vertical()
        .id_salt("history_scroll")
        .max_height(180.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for entry in &entries {
                widgets::card_hover(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(&entry.name)
                                .size(12.5)
                                .strong()
                                .color(TEXT),
                        );
                        widgets::chip(ui, &entry.scope, MUTED);
                        widgets::chip(ui, &util::human_size(entry.bytes), ACCENT);
                        widgets::chip(ui, &util::human_age(entry.moved_at), MUTED);
                        let active = winapi::junction_target(&entry.source_path()).is_some();
                        widgets::chip(
                            ui,
                            if active { "odkaz aktivní" } else { "odkaz chybí" },
                            if active { GOOD } else { ERROR },
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui
                                    .add_enabled(
                                        !busy,
                                        widgets::ghost_button("Vrátit na C:…"),
                                    )
                                    .clicked()
                                {
                                    pending = Some(RowAction::Restore(Box::new(entry.clone())));
                                }
                                if ui.add(widgets::ghost_button("Cíl")).clicked() {
                                    let target = entry.target_path();
                                    let path = if target.is_dir() { target } else { entry.source_path() };
                                    pending = Some(RowAction::Open(path));
                                }
                            },
                        );
                    });
                    ui.label(
                        egui::RichText::new(format!(
                            "{}  →  {}",
                            entry.source,
                            entry.target
                        ))
                        .size(10.0)
                        .monospace()
                        .color(MUTED),
                    );
                });
                ui.add_space(3.0);
            }
        });

    if let Some(RowAction::Restore(entry)) = pending {
        app.confirm_restore = Some(ConfirmRestore {
            entry: *entry,
            delete_target: false,
        });
    } else if let Some(RowAction::Open(path)) = pending {
        if let Err(err) = winapi::open_in_explorer(&path) {
            app.log_push(crate::app::LogKind::Error, err.to_string());
        }
    }
}