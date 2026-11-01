//! PathMover — move heavy AppData folders to another drive via NTFS junctions.
//!
//! Windows-only desktop app built with `egui`/`eframe`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(windows))]
compile_error!("PathMover is a Windows-only application (NTFS junctions, Win32 API).");

mod app;
mod cleaner;
mod config;
mod fsutil;
mod relocator;
mod scanner;
mod ui;
mod util;
mod winapi;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(app::APP_NAME)
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([900.0, 580.0])
            .with_icon(std::sync::Arc::new(ui::theme::make_icon())),
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        app::APP_NAME,
        options,
        Box::new(|cc| Ok(Box::new(app::PathMoverApp::new(cc)))),
    )
}