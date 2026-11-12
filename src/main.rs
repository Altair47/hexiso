// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

// Built as a console program so the command line (`hexiso info x.iso`, exit codes, pipes)
// works from terminals and scripts; when the window is opened the console is hidden.

#![allow(clippy::type_complexity, clippy::too_many_arguments)]

mod app;
mod cli;
mod dialogs;
mod drives;
mod fs_fat;
mod fs_iso;
mod fs_udf;
mod hxz;
mod icon_art;
mod jobs;
mod model;
mod ops;
mod selftest;
mod soft;
mod source;
mod theme;
mod writer;

use eframe::egui;
use std::path::{Path, PathBuf};

/// Hide the console for the window: free it if it's ours alone; if we were started from a
/// terminal, start the window as a separate process so the terminal isn't blocked.
#[cfg(windows)]
fn detach_console(screenshot: bool) {
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
    let mut ids = [0u32; 4];
    let n = unsafe { GetConsoleProcessList(ids.as_mut_ptr(), 4) };
    if n == 0 {
        return; // no console at all
    }
    if n == 1 {
        unsafe { FreeConsole() };
        return;
    }
    // shared console (a terminal): relaunch the window detached unless asked to stay
    if screenshot || std::env::var_os("HEXISO_FOREGROUND").is_some() {
        unsafe { FreeConsole() };
        return;
    }
    use std::os::windows::process::CommandExt;
    if let Ok(exe) = std::env::current_exe() {
        let ok = std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .env("HEXISO_FOREGROUND", "1")
            .creation_flags(0x0000_0008 | 0x0000_0200) // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
            .spawn()
            .is_ok();
        if ok {
            std::process::exit(0);
        }
    }
    unsafe { FreeConsole() };
}

fn main() -> eframe::Result<()> {
    let all: Vec<String> = std::env::args().skip(1).collect();
    let first = all.first().map(|s| s.as_str()).unwrap_or("");
    // window: no arguments, an existing file to open, or window options like --screenshot
    let is_gui = first.is_empty() || Path::new(first).is_file() || (first.starts_with("--") && !matches!(first, "--help" | "--version" | "--selftest"));
    let is_cli = !is_gui && (cli::COMMANDS.contains(&first) || matches!(first, "-h" | "--help" | "-V" | "--version" | "/?" | "--selftest") || !first.starts_with('-'));
    if is_cli {
        let mut args = all.clone();
        if first == "--selftest" {
            args[0] = "selftest".into();
        }
        if first == "/?" {
            args[0] = "help".into();
        }
        std::process::exit(cli::run(args));
    }
    #[cfg(windows)]
    detach_console(all.iter().any(|a| a == "--screenshot"));
    let mut screenshot: Option<PathBuf> = None;
    let mut open: Option<PathBuf> = None;
    let mut start_view: Option<String> = None;
    let mut start_lba: Option<u64> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--screenshot" {
            screenshot = args.next().map(PathBuf::from);
        } else if a == "--view" {
            start_view = args.next();
        } else if a == "--lba" {
            start_lba = args.next().and_then(|v| v.parse().ok());
        } else if !a.starts_with('-') {
            // only auto-open real image files; ignore anything else the launcher passes along
            let p = PathBuf::from(a.trim_matches('"'));
            if p.is_file() {
                open = Some(p);
            }
        }
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("HEXISO // disc image toolkit")
        .with_inner_size(
            std::env::var("HEXISO_SIZE")
                .ok()
                .and_then(|s| s.split_once('x').and_then(|(w, h)| Some([w.parse::<f32>().ok()?, h.parse::<f32>().ok()?])))
                .unwrap_or([1360.0, 820.0]),
        )
        .with_min_inner_size([480.0, 340.0])
        .with_drag_and_drop(true)
        .with_icon(std::sync::Arc::new(egui::IconData { rgba: icon_art::icon_rgba(64), width: 64, height: 64 }));
    if screenshot.is_some() {
        viewport = viewport.with_active(false);
        #[cfg(windows)]
        {
            viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }
    let setup = move |ctx: &egui::Context| {
        let mut a = app::HexApp::new(ctx, screenshot, open);
        if start_view.as_deref() == Some("sectors") {
            a.view = app::View::Sectors;
        }
        if let Some(l) = start_lba {
            a.hex_lba = l;
            a.hex_input = l.to_string();
        }
        a
    };
    // Renderer: GPU (wgpu) when there is a real graphics card, otherwise the built-in CPU
    // renderer, which is many times faster than Windows' software Direct3D driver.
    // HEXISO_RENDERER=cpu|gpu overrides the automatic choice.
    let use_cpu = match std::env::var("HEXISO_RENDERER").map(|s| s.to_lowercase()).as_deref() {
        Ok("cpu") | Ok("software") => true,
        Ok("gpu") | Ok("wgpu") => false,
        _ => !soft::has_hardware_gpu(),
    };
    if use_cpu {
        if let Err(e) = soft::run(viewport, setup) {
            eprintln!("hexiso: {}", e);
            std::process::exit(1);
        }
        return Ok(());
    }
    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions { viewport, ..Default::default() };
    // Smooth window dragging: every move event triggers a redraw; with the default (vsync, two
    // queued frames) each redraw blocks Windows' move loop waiting for the display, which feels
    // choppy, worst with software rendering (VMs, remote desktop). The window only redraws when
    // something happens, so presenting without waiting costs nothing while idle.
    // Set HEXISO_VSYNC=1 to get the old behaviour back.
    options.wgpu_options.surface = eframe::egui_wgpu::SurfaceConfig {
        present_mode: if std::env::var_os("HEXISO_VSYNC").is_some() { eframe::wgpu::PresentMode::AutoVsync } else { eframe::wgpu::PresentMode::AutoNoVsync },
        desired_maximum_frame_latency: Some(1),
    };
    #[cfg(target_os = "macos")]
    if screenshot.is_some() {
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        options.event_loop_builder = Some(Box::new(|b| {
            b.with_activate_ignoring_other_apps(false);
        }));
    }
    eframe::run_native(
        "hexiso",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(setup(&cc.egui_ctx)))
        }),
    )
}
