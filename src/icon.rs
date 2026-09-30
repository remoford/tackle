// tackle's icon, drawn in code at any size: a block "T" on a dark rounded tile. Each pixel
// is supersampled 4x4, so edges stay smooth at every size and DPI.

const TILE: [f32; 3] = [34.0, 40.0, 58.0];
const INK: [f32; 3] = [240.0, 150.0, 60.0];

/// What covers a point of the unit square: None (outside), Some(false) tile, Some(true) ink.
fn at(x: f32, y: f32) -> Option<bool> {
    let r = 0.16;
    let cx = x.clamp(r, 1.0 - r);
    let cy = y.clamp(r, 1.0 - r);
    if (x - cx).powi(2) + (y - cy).powi(2) > r * r {
        return None;
    }
    let bar = (0.19..0.81).contains(&x) && (0.19..0.35).contains(&y);
    let stem = (0.41..0.59).contains(&x) && (0.19..0.84).contains(&y);
    Some(bar || stem)
}

/// The icon as RGBA, `size` pixels square.
pub fn tackle(size: usize) -> Vec<u8> {
    const S: usize = 4;
    let mut px = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let (mut rgb, mut hits) = ([0.0f32; 3], 0);
            for sy in 0..S {
                for sx in 0..S {
                    let u = (x as f32 + (sx as f32 + 0.5) / S as f32) / size as f32;
                    let v = (y as f32 + (sy as f32 + 0.5) / S as f32) / size as f32;
                    if let Some(ink) = at(u, v) {
                        let c = if ink { INK } else { TILE };
                        for i in 0..3 {
                            rgb[i] += c[i];
                        }
                        hits += 1;
                    }
                }
            }
            if hits > 0 {
                let i = (y * size + x) * 4;
                for k in 0..3 {
                    px[i + k] = (rgb[k] / hits as f32) as u8;
                }
                px[i + 3] = (255 * hits / (S * S)) as u8;
            }
        }
    }
    px
}

/// Writes a multi-size .ico (16 to 256 pixels) for the exe's embedded icon.
pub fn write_ico(path: &std::path::Path) -> std::io::Result<()> {
    let sizes = [16usize, 24, 32, 48, 64, 128, 256];
    let images: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&n| {
            let px = tackle(n);
            let mut bmp = Vec::new();
            bmp.extend_from_slice(&40u32.to_le_bytes());
            bmp.extend_from_slice(&(n as u32).to_le_bytes());
            bmp.extend_from_slice(&(n as u32 * 2).to_le_bytes());
            bmp.extend_from_slice(&1u16.to_le_bytes());
            bmp.extend_from_slice(&32u16.to_le_bytes());
            bmp.extend_from_slice(&[0u8; 24]);
            for y in (0..n).rev() {
                for x in 0..n {
                    let i = (y * n + x) * 4;
                    bmp.extend_from_slice(&[px[i + 2], px[i + 1], px[i], px[i + 3]]);
                }
            }
            // The 1-bit AND mask, rows padded to 4 bytes; alpha does the real work.
            bmp.extend(std::iter::repeat(0u8).take(n.div_ceil(32) * 4 * n));
            bmp
        })
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (n, img) in sizes.iter().zip(&images) {
        out.push(if *n >= 256 { 0 } else { *n as u8 });
        out.push(if *n >= 256 { 0 } else { *n as u8 });
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend(img);
    }
    std::fs::write(path, out)
}
