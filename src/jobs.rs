// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! Background jobs with progress and cancel.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub type Apply = Box<dyn FnOnce(&mut crate::app::HexApp) + Send>;

#[derive(Default)]
pub struct Progress {
    done: AtomicU64,
    total: AtomicU64,
    cancel: AtomicBool,
    pub status: Mutex<String>,
}

impl Progress {
    pub fn set_total(&self, t: u64) {
        self.total.store(t, Ordering::Relaxed);
    }
    pub fn set_done(&self, d: u64) {
        self.done.store(d, Ordering::Relaxed);
    }
    pub fn add(&self, d: u64) {
        self.done.fetch_add(d, Ordering::Relaxed);
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn fraction(&self) -> f32 {
        let t = self.total.load(Ordering::Relaxed);
        if t == 0 {
            return 0.0;
        }
        (self.done.load(Ordering::Relaxed) as f64 / t as f64).min(1.0) as f32
    }
    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
    pub fn say(&self, s: impl Into<String>) {
        *self.status.lock().unwrap() = s.into();
    }
}

pub struct Job {
    pub title: String,
    pub prog: Arc<Progress>,
    pub started: Instant,
    pub result: Arc<Mutex<Option<Result<Apply, String>>>>,
}

impl Job {
    pub fn spawn<F>(title: &str, ctx: &eframe::egui::Context, f: F) -> Job
    where
        F: FnOnce(&Progress) -> Result<Apply, String> + Send + 'static,
    {
        let prog = Arc::new(Progress::default());
        let result = Arc::new(Mutex::new(None));
        let (p2, r2) = (prog.clone(), result.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&p2)))
                .unwrap_or_else(|_| Err("the operation crashed unexpectedly".to_string()));
            *r2.lock().unwrap() = Some(r);
            ctx.request_repaint();
        });
        Job { title: title.to_string(), prog, started: Instant::now(), result }
    }
    pub fn take(&self) -> Option<Result<Apply, String>> {
        self.result.lock().unwrap().take()
    }
}
