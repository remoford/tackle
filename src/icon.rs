pub const SIZE: usize = 32;

/// A block "T" on a dark rounded tile, as RGBA.
pub fn tackle() -> Vec<u8> {
    let mut px = vec![0u8; SIZE * SIZE * 4];
    let tile = [34, 40, 58, 255];
    let ink = [240, 150, 60, 255];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let corner = |a: usize| if a < 4 { 4 - a } else if a >= SIZE - 4 { a + 5 - SIZE } else { 0 };
            let (dx, dy) = (corner(x), corner(y));
            if dx * dx + dy * dy > 16 {
                continue;
            }
            let bar = (6..26).contains(&x) && (6..11).contains(&y);
            let stem = (13..19).contains(&x) && (6..27).contains(&y);
            let i = (y * SIZE + x) * 4;
            px[i..i + 4].copy_from_slice(if bar || stem { &ink } else { &tile });
        }
    }
    px
}
