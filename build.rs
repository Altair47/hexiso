// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

// Generates the program icon and embeds it (plus version info) into the Windows .exe.

include!("src/icon_art.rs");

fn png(size: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, size, size);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("png header");
        w.write_image_data(&icon_rgba(size)).expect("png data");
    }
    out
}

fn main() {
    println!("cargo:rerun-if-changed=src/icon_art.rs");
    println!("cargo:rerun-if-changed=build.rs");
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico = out_dir.join("hexiso.ico");
    // .ico with PNG-compressed entries
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let imgs: Vec<Vec<u8>> = sizes.iter().map(|s| png(*s)).collect();
    let mut f = Vec::new();
    f.extend_from_slice(&[0, 0, 1, 0]);
    f.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, d) in sizes.iter().zip(&imgs) {
        let b = if *s >= 256 { 0 } else { *s as u8 };
        f.extend_from_slice(&[b, b, 0, 0]);
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&32u16.to_le_bytes());
        f.extend_from_slice(&(d.len() as u32).to_le_bytes());
        f.extend_from_slice(&offset.to_le_bytes());
        offset += d.len() as u32;
    }
    for d in &imgs {
        f.extend_from_slice(d);
    }
    std::fs::write(&ico, &f).expect("write ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon(ico.to_str().unwrap());
        res.set("ProductName", "HEXISO");
        res.set("FileDescription", "HEXISO disc image toolkit");
        res.set("OriginalFilename", "hexiso.exe");
        res.set("LegalCopyright", "Copyright (C) Nick Petropoulos. AGPL-3.0");
        res.set("CompanyName", "Nick Petropoulos");
        if let Err(e) = res.compile() {
            println!("cargo:warning=program icon not embedded: {}", e);
        }
    }
}
