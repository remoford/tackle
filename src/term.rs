// Draws a vt100 screen with the egui painter and turns egui input into terminal bytes.

use eframe::egui::{self, Color32, FontId, Key, Pos2, Rect, Stroke, Vec2};

pub const FG: Color32 = Color32::from_rgb(204, 204, 204);
pub const BG: Color32 = Color32::from_rgb(18, 18, 22);

pub struct Metrics {
    pub cell: Vec2,
    pub font: FontId,
}

pub fn metrics(ctx: &egui::Context, size: f32) -> Metrics {
    let font = FontId::monospace(size);
    let (w, h) = ctx.fonts(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
    Metrics { cell: Vec2::new(w, h), font }
}

fn indexed(i: u8) -> Color32 {
    const BASE: [(u8, u8, u8); 16] = [
        (12, 12, 12), (197, 15, 31), (19, 161, 14), (193, 156, 0), (0, 55, 218), (136, 23, 152), (58, 150, 221), (204, 204, 204),
        (118, 118, 118), (231, 72, 86), (22, 198, 12), (249, 241, 165), (59, 120, 255), (180, 0, 158), (97, 214, 214), (242, 242, 242),
    ];
    match i {
        0..=15 => {
            let (r, g, b) = BASE[i as usize];
            Color32::from_rgb(r, g, b)
        }
        16..=231 => {
            let i = i - 16;
            let c = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color32::from_rgb(c(i / 36), c(i / 6 % 6), c(i % 6))
        }
        _ => {
            let g = 8 + (i - 232) * 10;
            Color32::from_rgb(g, g, g)
        }
    }
}

fn color(c: vt100::Color, default: Color32) -> Color32 {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => indexed(i),
        vt100::Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
    }
}

pub fn paint(painter: &egui::Painter, rect: Rect, m: &Metrics, screen: &vt100::Screen, focused: bool) {
    painter.rect_filled(rect, 0.0, BG);
    let (rows, cols) = screen.size();
    let at = |r: u16, c: u16| Pos2::new(rect.min.x + c as f32 * m.cell.x, rect.min.y + r as f32 * m.cell.y);
    for r in 0..rows {
        // ASCII runs of one colour go out as one string; anything else is placed cell by
        // cell so fallback-font glyphs can't push the grid out of line.
        let mut run = String::new();
        let mut run_col = 0u16;
        let mut run_fg = FG;
        let flush = |run: &mut String, col: u16, fg: Color32| {
            if !run.is_empty() {
                painter.text(at(r, col), egui::Align2::LEFT_TOP, std::mem::take(run), m.font.clone(), fg);
            }
        };
        let mut c = 0;
        while c < cols {
            let Some(cell) = screen.cell(r, c) else { break };
            if cell.is_wide_continuation() {
                c += 1;
                continue;
            }
            let width = if cell.is_wide() { 2 } else { 1 };
            let (mut fg, mut bg) = (color(cell.fgcolor(), FG), color(cell.bgcolor(), BG));
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }
            if bg != BG {
                painter.rect_filled(Rect::from_min_size(at(r, c), Vec2::new(m.cell.x * width as f32, m.cell.y)), 0.0, bg);
            }
            if cell.underline() {
                let y = at(r, c).y + m.cell.y - 1.0;
                painter.hline(at(r, c).x..=at(r, c + width).x, y, Stroke::new(1.0, fg));
            }
            let s = cell.contents();
            if !s.is_empty() && s != " " {
                if s.is_ascii() && fg == run_fg && run_col + run.len() as u16 == c {
                    run.push_str(&s);
                } else {
                    flush(&mut run, run_col, run_fg);
                    if s.is_ascii() {
                        run = s;
                        run_col = c;
                        run_fg = fg;
                    } else {
                        painter.text(at(r, c), egui::Align2::LEFT_TOP, s, m.font.clone(), fg);
                    }
                }
            }
            c += width;
        }
        flush(&mut run, run_col, run_fg);
    }
    if !screen.hide_cursor() && screen.scrollback() == 0 {
        let (r, c) = screen.cursor_position();
        let cur = Rect::from_min_size(at(r, c), m.cell);
        if focused {
            painter.rect_filled(cur, 0.0, Color32::from_rgba_unmultiplied(204, 204, 204, 110));
        } else {
            painter.rect_stroke(cur, 0.0, Stroke::new(1.0, FG));
        }
    }
}

fn key_bytes(key: Key, mods: egui::Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    let csi = |s: &str| format!("\x1b[{}", s).into_bytes();
    let arrow = |ch: char| {
        if mods.ctrl {
            format!("\x1b[1;5{}", ch).into_bytes()
        } else if mods.shift {
            format!("\x1b[1;2{}", ch).into_bytes()
        } else if app_cursor {
            format!("\x1bO{}", ch).into_bytes()
        } else {
            format!("\x1b[{}", ch).into_bytes()
        }
    };
    Some(match key {
        Key::Enter if mods.shift || mods.alt => b"\x1b\r".to_vec(),
        Key::Enter => b"\r".to_vec(),
        Key::Tab if mods.shift => csi("Z"),
        Key::Tab => b"\t".to_vec(),
        Key::Backspace if mods.ctrl => b"\x08".to_vec(),
        Key::Backspace => b"\x7f".to_vec(),
        Key::Escape => b"\x1b".to_vec(),
        Key::ArrowUp => arrow('A'),
        Key::ArrowDown => arrow('B'),
        Key::ArrowRight => arrow('C'),
        Key::ArrowLeft => arrow('D'),
        Key::Home => csi("H"),
        Key::End => csi("F"),
        Key::PageUp => csi("5~"),
        Key::PageDown => csi("6~"),
        Key::Delete => csi("3~"),
        Key::Insert => csi("2~"),
        _ if mods.ctrl && !mods.alt => {
            // Ctrl+C/V/X arrive as Copy/Paste/Cut events instead.
            let name = key.name();
            let ch = name.chars().next().filter(|c| name.len() == 1 && c.is_ascii_alphabetic())?;
            if matches!(ch, 'C' | 'V' | 'X') {
                return None;
            }
            vec![ch as u8 & 0x1f]
        }
        _ => return None,
    })
}

/// A key by name, as hr sends them: "enter", "esc", "tab", "shift+tab", "backspace",
/// "up", "down", "left", "right", "home", "end", "pageup", "pagedown", "delete", "space",
/// "ctrl+<letter>", or a single character.
pub fn named_key(name: &str) -> Option<Vec<u8>> {
    let n = name.trim().to_ascii_lowercase();
    let none = egui::Modifiers::NONE;
    let key = |k: Key, m: egui::Modifiers| key_bytes(k, m, false);
    match n.as_str() {
        "enter" | "return" => key(Key::Enter, none),
        "esc" | "escape" => key(Key::Escape, none),
        "tab" => key(Key::Tab, none),
        "shift+tab" => key(Key::Tab, egui::Modifiers::SHIFT),
        "backspace" => key(Key::Backspace, none),
        "up" => key(Key::ArrowUp, none),
        "down" => key(Key::ArrowDown, none),
        "left" => key(Key::ArrowLeft, none),
        "right" => key(Key::ArrowRight, none),
        "home" => key(Key::Home, none),
        "end" => key(Key::End, none),
        "pageup" => key(Key::PageUp, none),
        "pagedown" => key(Key::PageDown, none),
        "delete" => key(Key::Delete, none),
        "space" => Some(b" ".to_vec()),
        _ => match n.strip_prefix("ctrl+") {
            Some(c) if c.len() == 1 && c.as_bytes()[0].is_ascii_alphabetic() => Some(vec![c.as_bytes()[0].to_ascii_uppercase() & 0x1f]),
            Some(_) => None,
            None if name.chars().count() == 1 => Some(name.as_bytes().to_vec()),
            None => None,
        },
    }
}

pub fn input(events: &[egui::Event], app_cursor: bool, bracketed: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for e in events {
        match e {
            egui::Event::Text(t) => out.extend_from_slice(t.as_bytes()),
            egui::Event::Paste(t) => {
                let t = t.replace("\r\n", "\n");
                if bracketed {
                    out.extend_from_slice(b"\x1b[200~");
                    out.extend_from_slice(t.as_bytes());
                    out.extend_from_slice(b"\x1b[201~");
                } else {
                    out.extend_from_slice(t.replace('\n', "\r").as_bytes());
                }
            }
            egui::Event::Copy => out.push(0x03),
            egui::Event::Cut => out.push(0x18),
            egui::Event::Key { key, pressed: true, modifiers, .. } => {
                if let Some(b) = key_bytes(*key, *modifiers, app_cursor) {
                    out.extend(b);
                }
            }
            _ => {}
        }
    }
    out
}
