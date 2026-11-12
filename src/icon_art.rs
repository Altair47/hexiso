// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

// Self-contained pixel-art icon (used by build.rs for the .exe icon and by the app for the window icon).
// No imports: build.rs includes this file directly.

/// RGBA pixels of a `size` x `size` icon.
#[allow(dead_code)]
pub fn icon_rgba(size: u32) -> Vec<u8> {
    const H: [&str; 7] = ["#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"];
    const X: [&str; 7] = ["#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#"];
    let n = size as usize;
    let mut px = vec![0u8; n * n * 4];
    let cell = n as f32 / 16.0;
    for y in 0..n {
        for x in 0..n {
            let gx = (x as f32 / cell) as i32;
            let gy = (y as f32 / cell) as i32;
            // rounded dark tile
            let r = cell * 2.0;
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let (lo, hi) = (r, n as f32 - r);
            let cx = fx.clamp(lo, hi);
            let cy = fy.clamp(lo, hi);
            let inside = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() <= r;
            if !inside {
                continue;
            }
            let mut c = [4u8, 10, 12, 255];
            let edge = gx == 0 || gy == 0 || gx == 15 || gy == 15;
            if edge {
                c = [0, 150, 96, 255];
            }
            let glyph = |g: &[&str; 7], ox: i32| -> bool {
                let (lx, ly) = (gx - ox, gy - 3);
                (0..5).contains(&lx) && (0..7).contains(&ly) && g[ly as usize].as_bytes()[lx as usize] == b'#'
            };
            if glyph(&H, 2) || glyph(&X, 9) {
                let t = (gy - 3) as f32 / 6.0;
                c = [0, (255.0 - 40.0 * t) as u8, (156.0 + 80.0 * t) as u8, 255];
            }
            if gy == 12 && (2..14).contains(&gx) {
                c = if gx < 10 { [0, 217, 255, 255] } else { [20, 52, 44, 255] };
            }
            let i = (y * n + x) * 4;
            px[i..i + 4].copy_from_slice(&c);
        }
    }
    px
}
