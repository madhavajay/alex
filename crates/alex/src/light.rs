use std::io::{IsTerminal, Read as _, Write as _};

use anyhow::{bail, Result};
use flate2::read::GzDecoder;

const FRAMES_PACK: &[u8] = include_bytes!("light-frames.bin");
const LOGO_GZ: &[u8] = include_bytes!("light-logo.bin");
const FADE_ZONE: f64 = 0.14;
const BOTTOM_FADE_ZONE: f64 = 0.05;
const LOGO_WIDTH_FRAC: f64 = 0.72;

struct Logo {
    w: usize,
    h: usize,
    rgba: Vec<u8>,
}

fn load_logo() -> Result<Logo> {
    let mut raw = Vec::new();
    GzDecoder::new(LOGO_GZ).read_to_end(&mut raw)?;
    if raw.len() < 8 || &raw[..4] != b"ALGO" {
        bail!("bad logo header");
    }
    let w = u16::from_le_bytes([raw[4], raw[5]]) as usize;
    let h = u16::from_le_bytes([raw[6], raw[7]]) as usize;
    let rgba = raw.split_off(8);
    if rgba.len() != w * h * 4 {
        bail!("truncated logo data");
    }
    Ok(Logo { w, h, rgba })
}

struct Film {
    w: usize,
    h: usize,
    frames: usize,
    fps: u8,
    data: Vec<u8>,
}

fn load() -> Result<Film> {
    if FRAMES_PACK.len() < 7 || &FRAMES_PACK[..4] != b"ALXJ" {
        bail!("bad film header");
    }
    let frames = u16::from_le_bytes([FRAMES_PACK[4], FRAMES_PACK[5]]) as usize;
    let fps = FRAMES_PACK[6];
    let mut offset = 7usize;
    let mut data = Vec::new();
    let (mut w, mut h) = (0usize, 0usize);
    for _ in 0..frames {
        let len = u32::from_le_bytes([
            FRAMES_PACK[offset],
            FRAMES_PACK[offset + 1],
            FRAMES_PACK[offset + 2],
            FRAMES_PACK[offset + 3],
        ]) as usize;
        offset += 4;
        let mut decoder = jpeg_decoder::Decoder::new(&FRAMES_PACK[offset..offset + len]);
        let pixels = decoder.decode()?;
        let info = decoder.info().ok_or_else(|| anyhow::anyhow!("no jpeg info"))?;
        if w == 0 {
            w = info.width as usize;
            h = info.height as usize;
        } else if info.width as usize != w || info.height as usize != h {
            bail!("inconsistent frame sizes");
        }
        data.extend_from_slice(&pixels);
        offset += len;
    }
    if w == 0 || data.len() != w * h * 3 * frames {
        bail!("truncated film data");
    }
    Ok(Film {
        w,
        h,
        frames,
        fps,
        data,
    })
}

fn fade_axis(pos: usize, size: usize, lead_zone: f64, tail_zone: f64) -> f64 {
    if size == 0 {
        return 1.0;
    }
    let lead = (size as f64 * lead_zone).max(1.0);
    let tail = (size as f64 * tail_zone).max(1.0);
    let from_start = pos as f64;
    let from_end = (size - 1 - pos) as f64;
    let a = if from_start >= lead {
        1.0
    } else {
        (from_start / lead).powf(1.4)
    };
    let b = if from_end >= tail {
        1.0
    } else {
        (from_end / tail).powf(1.4)
    };
    a.min(b)
}

fn edge_fade(x: usize, w: usize, y: usize, h: usize) -> f64 {
    fade_axis(x, w, FADE_ZONE, FADE_ZONE) * fade_axis(y, h, FADE_ZONE, BOTTOM_FADE_ZONE)
}

#[allow(clippy::too_many_arguments)]
fn sample(
    film: &Film,
    frame: &[u8],
    scale: f64,
    src_x0f: f64,
    src_y0f: f64,
    out_w: usize,
    out_h: usize,
    x: usize,
    y: usize,
) -> (u8, u8, u8) {
    let x0 = ((src_x0f + x as f64 / scale) as usize).min(film.w - 1);
    let y0 = ((src_y0f + y as f64 / scale) as usize).min(film.h - 1);
    let x1 = ((src_x0f + (x + 1) as f64 / scale).ceil() as usize).clamp(x0 + 1, film.w);
    let y1 = ((src_y0f + (y + 1) as f64 / scale).ceil() as usize).clamp(y0 + 1, film.h);
    let (mut r, mut g, mut b, mut count) = (0u32, 0u32, 0u32, 0u32);
    for sy in y0..y1 {
        for sx in x0..x1 {
            let o = (sy * film.w + sx) * 3;
            r += frame[o] as u32;
            g += frame[o + 1] as u32;
            b += frame[o + 2] as u32;
            count += 1;
        }
    }
    let f = edge_fade(x, out_w, y, out_h);
    (
        ((r / count) as f64 * f) as u8,
        ((g / count) as f64 * f) as u8,
        ((b / count) as f64 * f) as u8,
    )
}

fn logo_alpha(index: usize, frames: usize) -> f64 {
    let start = frames * 7 / 10;
    let dur = (frames / 5).max(1);
    ((index as f64 - start as f64) / dur as f64).clamp(0.0, 1.0)
}

fn draw_logo_on_canvas(
    canvas: &mut [u8],
    cw: usize,
    ch: usize,
    logo: &Logo,
    alpha: f64,
    film_bottom: usize,
) {
    let target_w = (cw as f64 * LOGO_WIDTH_FRAC).min(cw as f64 - 2.0).max(10.0);
    let scale_l = target_w / logo.w as f64;
    let target_h = (logo.h as f64 * scale_l).max(2.0);
    let x_start = ((cw as f64 - target_w) / 2.0) as usize;
    let lowest_start = (ch.saturating_sub(2) as f64 - target_h).max(0.0);
    let below_image = (film_bottom + 3) as f64;
    let y_start = below_image.min(lowest_start).max(0.0) as usize;
    let y_end = ((y_start as f64 + target_h) as usize).min(ch);
    for fy in y_start..y_end {
        let dy = fy - y_start;
        let ly0 = (dy as f64 / scale_l) as usize;
        let ly1 = (((dy + 1) as f64 / scale_l).ceil() as usize).clamp(ly0 + 1, logo.h);
        if ly0 >= logo.h {
            break;
        }
        for dx in 0..target_w as usize {
            let fx = x_start + dx;
            if fx >= cw {
                break;
            }
            let lx0 = (dx as f64 / scale_l) as usize;
            let lx1 = (((dx + 1) as f64 / scale_l).ceil() as usize).clamp(lx0 + 1, logo.w);
            if lx0 >= logo.w {
                break;
            }
            let (mut r, mut g, mut b, mut a, mut count) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for ly in ly0..ly1.min(logo.h) {
                for lx in lx0..lx1.min(logo.w) {
                    let o = (ly * logo.w + lx) * 4;
                    r += logo.rgba[o] as u32;
                    g += logo.rgba[o + 1] as u32;
                    b += logo.rgba[o + 2] as u32;
                    a += logo.rgba[o + 3] as u32;
                    count += 1;
                }
            }
            if count == 0 {
                continue;
            }
            let blend = (a as f64 / count as f64 / 255.0) * alpha;
            if blend <= 0.003 {
                continue;
            }
            let o = (fy * cw + fx) * 3;
            for (c, v) in [(0usize, r), (1, g), (2, b)] {
                let src = canvas[o + c] as f64;
                canvas[o + c] = (src * (1.0 - blend) + (v / count) as f64 * blend) as u8;
            }
        }
    }
}

fn render_blocks(
    film: &Film,
    logo: &Logo,
    index: usize,
    cols: usize,
    rows: usize,
    status: &[String],
) -> String {
    let cw = cols.max(10);
    let text_rows = if rows >= 18 { status.len().min(3) } else { 0 };
    let ch = (rows.saturating_sub(1 + text_rows).max(5)) * 2;
    let mut canvas = vec![0u8; cw * ch * 3];
    let scale = (cw as f64 / film.w as f64).min(2.0);
    let fw = ((film.w as f64 * scale) as usize).min(cw);
    let full_fh = ((film.h as f64 * scale) as usize).max(2);
    let xoff = (cw - fw) / 2;
    let frame = &film.data[index * film.w * film.h * 3..];
    for y in 0..full_fh.min(ch) {
        for x in 0..fw {
            let (r, g, b) = sample(film, frame, scale, 0.0, 0.0, fw, full_fh, x, y);
            let o = (y * cw + x + xoff) * 3;
            canvas[o] = r;
            canvas[o + 1] = g;
            canvas[o + 2] = b;
        }
    }
    let alpha = logo_alpha(index, film.frames);
    if alpha > 0.0 {
        draw_logo_on_canvas(&mut canvas, cw, ch, logo, alpha, full_fh.min(ch));
    }
    let mut out = String::with_capacity(cw * ch * 12);
    out.push_str("\x1b[H");
    for row in 0..ch / 2 {
        out.push_str("\x1b[48;2;0;0;0m\x1b[2K");
        let mut last = (300u16, 0u16, 0u16, 300u16, 0u16, 0u16);
        for x in 0..cw {
            let t = (row * 2 * cw + x) * 3;
            let b = ((row * 2 + 1) * cw + x) * 3;
            let key = (
                canvas[t] as u16,
                canvas[t + 1] as u16,
                canvas[t + 2] as u16,
                canvas[b] as u16,
                canvas[b + 1] as u16,
                canvas[b + 2] as u16,
            );
            if key != last {
                out.push_str(&format!(
                    "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m",
                    key.0, key.1, key.2, key.3, key.4, key.5
                ));
                last = key;
            }
            out.push('▀');
        }
        out.push_str("\x1b[0m\n");
    }
    for line in status.iter().rev().take(text_rows).rev() {
        let clipped: String = line.chars().take(cw.saturating_sub(4)).collect();
        out.push_str(&format!(
            "\x1b[48;2;0;0;0m\x1b[2K  \x1b[2m\x1b[38;5;180m{clipped}\x1b[0m\n"
        ));
    }
    out
}

fn kitty_frame_payload(film: &Film, logo: &Logo, index: usize) -> String {
    use base64::Engine;
    use std::io::Write as _;
    let start = index * film.w * film.h * 3;
    let mut rgb = film.data[start..start + film.w * film.h * 3].to_vec();
    let alpha = logo_alpha(index, film.frames);
    if alpha > 0.0 {
        draw_logo_on_canvas(&mut rgb, film.w, film.h, logo, alpha, film.h);
    }
    let mut enc =
        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    let _ = enc.write_all(&rgb);
    let compressed = enc.finish().unwrap_or_default();
    base64::engine::general_purpose::STANDARD.encode(compressed)
}

fn anim_cells(film: &Film, cols: usize, rows: usize, text_rows: usize) -> (usize, usize) {
    let max_r = rows.saturating_sub(1 + text_rows).max(3);
    let mut c = cols.max(10);
    let mut r = ((c as f64) * film.h as f64 / film.w as f64 * 0.5).round() as usize;
    if r > max_r {
        r = max_r;
        c = (((r as f64 / 0.5) * film.w as f64) / film.h as f64).round() as usize;
        c = c.clamp(10, cols.max(10));
    }
    (c, r.max(1))
}

fn render_kitty_virtual(
    film: &Film,
    payload: &str,
    cols: usize,
    rows: usize,
    status: &[String],
) -> String {
    let text_rows = if rows >= 18 { status.len().min(3) } else { 0 };
    let (c, r) = anim_cells(film, cols, rows, text_rows);
    let c = c.min(DIACRITICS.len());
    let r = r.min(DIACRITICS.len());
    let pad = " ".repeat(cols.saturating_sub(c) / 2);
    let mut out = String::with_capacity(payload.len() + c * r * 8);
    out.push_str("\x1b[H");
    out.push_str(&kitty_transmit(
        &format!(
            "a=T,U=1,t=d,f=24,o=z,s={},v={},i=43,c={c},r={r},q=2",
            film.w, film.h
        ),
        payload,
    ));
    out.push_str(&virtual_grid(43, c, r, &pad));
    for line in status.iter().rev().take(text_rows).rev() {
        let clipped: String = line.chars().take(cols.saturating_sub(4)).collect();
        out.push_str(&format!(
            "\x1b[48;2;0;0;0m\x1b[2K  \x1b[2m\x1b[38;5;180m{clipped}\x1b[0m\n"
        ));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn render_kitty(
    film: &Film,
    payload: &str,
    cols: usize,
    rows: usize,
    status: &[String],
) -> String {
    let text_rows = if rows >= 18 { status.len().min(3) } else { 0 };
    let max_r = rows.saturating_sub(1 + text_rows).max(3);
    let mut c = cols.max(10);
    let mut r = ((c as f64) * film.h as f64 / film.w as f64 * 0.5).round() as usize;
    if r > max_r {
        r = max_r;
        c = (((r as f64 / 0.5) * film.w as f64) / film.h as f64).round() as usize;
        c = c.clamp(10, cols.max(10));
    }
    let pad = " ".repeat(cols.saturating_sub(c) / 2);
    let mut out = String::with_capacity(payload.len() + 4096);
    out.push_str("\x1b[H");
    out.push_str(&pad);
    let bytes = payload.as_bytes();
    let chunks: Vec<&[u8]> = bytes.chunks(4096).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = if i + 1 == chunks.len() { 0 } else { 1 };
        out.push_str("\x1b_G");
        if i == 0 {
            out.push_str(&format!(
                "a=T,t=d,f=24,o=z,s={},v={},i=4243,p=1,c={c},r={r},C=1,q=2,",
                film.w, film.h
            ));
        }
        out.push_str(&format!("m={more};"));
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push_str("\x1b\\");
    }
    out.push_str(&format!("\x1b[{}H", r + 1));
    for line in status.iter().rev().take(text_rows).rev() {
        let clipped: String = line.chars().take(cols.saturating_sub(4)).collect();
        out.push_str(&format!(
            "\x1b[48;2;0;0;0m\x1b[2K  \x1b[2m\x1b[38;5;180m{clipped}\x1b[0m\n"
        ));
    }
    out
}

fn read_status(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|s| {
            s.lines()
                .filter(|l| !l.trim().is_empty())
                .rev()
                .take(3)
                .map(String::from)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        })
        .unwrap_or_default()
}

pub async fn run(loops: u32, forever: bool, follow: Option<std::path::PathBuf>) -> Result<()> {
    if !std::io::stdout().is_terminal() {
        bail!("alexandria light needs an interactive terminal");
    }
    if std::env::var("NO_COLOR").is_ok() {
        bail!("alexandria light needs color (NO_COLOR is set)");
    }
    let film = load()?;
    let logo = load_logo()?;
    let delay =
        std::time::Duration::from_millis((1000.0 / (film.fps.max(1) as f64 * 1.5)) as u64);
    print!("\x1b[?1049h\x1b[?25l\x1b[48;2;0;0;0m\x1b[2J");
    let _ = std::io::stdout().flush();
    let result = play(&film, &logo, loops, forever, delay, follow.as_deref()).await;
    print!("\x1b[0m\x1b[?25h\x1b[?1049l");
    let _ = std::io::stdout().flush();
    result
}

async fn play(
    film: &Film,
    logo: &Logo,
    loops: u32,
    forever: bool,
    delay: std::time::Duration,
    follow: Option<&std::path::Path>,
) -> Result<()> {
    let mut remaining = loops.max(1);
    let proto = graphics_protocol();
    let use_kitty = proto == Some("kitty");
    let use_virtual = proto == Some("kitty-virtual");
    let mut payload_cache: Vec<Option<String>> = vec![None; film.frames];
    #[cfg(unix)]
    let mut term =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
    loop {
        for i in 0..film.frames {
            let (cols, rows) = crossterm::terminal::size()
                .map(|(c, r)| (c as usize, r as usize))
                .unwrap_or((80, 24));
            let status = follow.map(read_status).unwrap_or_default();
            if use_kitty || use_virtual {
                if payload_cache[i].is_none() {
                    payload_cache[i] = Some(kitty_frame_payload(film, logo, i));
                }
                let payload = payload_cache[i].as_deref().unwrap_or("");
                if use_virtual {
                    print!("{}", render_kitty_virtual(film, payload, cols, rows, &status));
                } else {
                    print!("{}", render_kitty(film, payload, cols, rows, &status));
                }
            } else {
                print!("{}", render_blocks(film, logo, i, cols, rows, &status));
            }
            let _ = std::io::stdout().flush();
            #[cfg(unix)]
            {
                let sig = async {
                    match term.as_mut() {
                        Some(t) => {
                            t.recv().await;
                        }
                        None => std::future::pending::<()>().await,
                    }
                };
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = tokio::signal::ctrl_c() => return Ok(()),
                    _ = sig => return Ok(()),
                }
            }
            #[cfg(not(unix))]
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = tokio::signal::ctrl_c() => return Ok(()),
            }
        }
        if !forever {
            remaining -= 1;
            if remaining == 0 {
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn film_loads() {
        let film = load().unwrap();
        assert_eq!(film.w, 720);
        assert_eq!(film.h, 360);
        assert!(film.frames > 10);
        assert!(film.fps > 0);
    }

    #[test]
    fn logo_loads_and_fades() {
        let logo = load_logo().unwrap();
        assert_eq!(logo.w, 800);
        assert!(logo.h > 50);
        assert_eq!(logo.rgba.len(), logo.w * logo.h * 4);
        assert_eq!(logo_alpha(0, 60), 0.0);
        assert_eq!(logo_alpha(42, 60), 0.0);
        assert_eq!(logo_alpha(54, 60), 1.0);
        assert!(logo_alpha(48, 60) > 0.0 && logo_alpha(48, 60) < 1.0);
    }

    #[test]
    fn fade_is_full_in_center_zero_at_edge() {
        assert!(edge_fade(0, 100, 50, 100) < 0.05);
        assert!((edge_fade(50, 100, 50, 100) - 1.0).abs() < f64::EPSILON);
        assert!(edge_fade(99, 100, 50, 100) < 0.05);
        assert!(edge_fade(50, 100, 99, 100) < 0.5);
    }

    #[test]
    fn renders_within_bounds() {
        let film = load().unwrap();
        let logo = load_logo().unwrap();
        let s = render_blocks(&film, &logo, 55, 60, 20, &["building".to_string()]);
        assert!(s.contains('▀'));
        for line in s.lines() {
            let visible = line
                .chars()
                .filter(|c| *c == '▀' || *c == ' ')
                .count();
            assert!(visible <= 60);
        }
    }
}

const LOGO_PNG: &[u8] = include_bytes!("light-logo.png");

const PLACEHOLDER: char = '\u{10EEEE}';
const DIACRITICS: &[u32] = &[
    0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F, 0x0346, 0x034A,
    0x034B, 0x034C, 0x0350, 0x0351, 0x0352, 0x0357, 0x035B, 0x0363, 0x0364, 0x0365,
    0x0366, 0x0367, 0x0368, 0x0369, 0x036A, 0x036B, 0x036C, 0x036D, 0x036E, 0x036F,
    0x0483, 0x0484, 0x0485, 0x0486, 0x0487, 0x0592, 0x0593, 0x0594, 0x0595, 0x0597,
    0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1, 0x05A8, 0x05A9,
    0x05AB, 0x05AC, 0x05AF, 0x05C4, 0x0610, 0x0611, 0x0612, 0x0613, 0x0614, 0x0615,
    0x0616, 0x0617, 0x0657, 0x0658, 0x0659, 0x065A, 0x065B, 0x065D, 0x065E, 0x06D6,
    0x06D7, 0x06D8, 0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0, 0x06E1, 0x06E2,
    0x06E4, 0x06E7, 0x06E8, 0x06EB, 0x06EC, 0x0730, 0x0732, 0x0733, 0x0735, 0x0736,
    0x073A, 0x073D, 0x073F, 0x0740, 0x0741, 0x0743, 0x0745, 0x0747, 0x0749, 0x074A,
    0x07EB, 0x07EC, 0x07ED, 0x07EE, 0x07EF, 0x07F0, 0x07F1, 0x07F3, 0x0816, 0x0817,
    0x0818, 0x0819, 0x081B, 0x081C, 0x081D, 0x081E, 0x081F, 0x0820, 0x0821, 0x0822,
    0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A, 0x082B, 0x082C, 0x082D, 0x0951,
    0x0953, 0x0954, 0x0F82, 0x0F83, 0x0F86, 0x0F87, 0x135D, 0x135E, 0x135F, 0x17DD,
    0x193A, 0x1A17, 0x1A75, 0x1A76, 0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C,
    0x1B6B, 0x1B6D, 0x1B6E, 0x1B6F, 0x1B70, 0x1B71, 0x1B72, 0x1B73, 0x1CD0, 0x1CD1,
    0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4, 0x1DC5, 0x1DC6,
    0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1, 0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5,
    0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9, 0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF,
    0x1DE0, 0x1DE1, 0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1,
    0x20D4, 0x20D5, 0x20D6, 0x20D7, 0x20DB, 0x20DC, 0x20E1, 0x20E7, 0x20E9, 0x20F0,
    0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2, 0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6,
    0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA, 0x2DEB, 0x2DEC, 0x2DED, 0x2DEE, 0x2DEF, 0x2DF0,
    0x2DF1, 0x2DF2, 0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA,
    0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D, 0xA6F0, 0xA6F1,
    0xA8E0, 0xA8E1, 0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5, 0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9,
    0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED, 0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2,
    0xAAB3, 0xAAB7, 0xAAB8, 0xAABE, 0xAABF, 0xAAC1, 0xFE20, 0xFE21, 0xFE22, 0xFE23,
    0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38, 0x1D185, 0x1D186, 0x1D187, 0x1D188, 0x1D189,
    0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243, 0x1D244,
];

fn in_herdr() -> bool {
    std::env::var("HERDR_PANE_ID").is_ok()
        || std::env::var("HERDR_SOCKET_PATH").is_ok()
        || std::env::var("HERDR_ENV").is_ok()
}

fn virtual_grid(id: u32, c: usize, r: usize, pad: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("\x1b[38;5;{id}m"));
    for row in 0..r {
        out.push_str(pad);
        for col in 0..c {
            out.push(PLACEHOLDER);
            if let (Some(rd), Some(cd)) = (
                DIACRITICS.get(row).and_then(|v| char::from_u32(*v)),
                DIACRITICS.get(col).and_then(|v| char::from_u32(*v)),
            ) {
                out.push(rd);
                out.push(cd);
            }
        }
        out.push('\n');
    }
    out.push_str("\x1b[39m");
    out
}

fn kitty_transmit(control: &str, b64: &str) -> String {
    let bytes = b64.as_bytes();
    let chunks: Vec<&[u8]> = bytes.chunks(4096).collect();
    let mut out = String::with_capacity(b64.len() + chunks.len() * 16);
    for (i, chunk) in chunks.iter().enumerate() {
        let more = if i + 1 == chunks.len() { 0 } else { 1 };
        out.push_str("\x1b_G");
        if i == 0 {
            out.push_str(control);
            out.push(',');
        }
        out.push_str(&format!("m={more};"));
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push_str("\x1b\\");
    }
    out
}

fn kitty_virtual_logo(cols: usize) -> String {
    use base64::Engine;
    let (c, pad) = logo_cells(cols);
    let c = c.min(DIACRITICS.len());
    let rows = ((c as f64 * 0.155 * 0.55).ceil() as usize)
        .clamp(3, 12)
        .min(DIACRITICS.len());
    let b64 = base64::engine::general_purpose::STANDARD.encode(LOGO_PNG);
    let mut out = kitty_transmit(
        &format!("a=T,U=1,f=100,t=d,i=42,c={c},r={rows},q=2"),
        &b64,
    );
    out.push_str(&virtual_grid(42, c, rows, &pad));
    out
}

fn kitty_probe() -> bool {
    if std::env::var("TMUX").is_ok() {
        return false;
    }
    if crossterm::terminal::enable_raw_mode().is_err() {
        return false;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        let Ok(mut tty) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
        else {
            let _ = tx.send(false);
            return;
        };
        let _ = tty.write_all(b"\x1b_Gi=4242,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c");
        let _ = tty.flush();
        let mut buf = [0u8; 512];
        let mut acc: Vec<u8> = Vec::new();
        loop {
            match tty.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    acc.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&acc);
                    if text.contains("\x1b[?") && text.ends_with('c') {
                        let _ = tx.send(text.contains("_Gi=4242"));
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(false);
    });
    let ok = rx
        .recv_timeout(std::time::Duration::from_millis(400))
        .unwrap_or(false);
    let _ = crossterm::terminal::disable_raw_mode();
    ok
}

fn kitty_supported() -> bool {
    static PROBE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PROBE.get_or_init(kitty_probe)
}

fn graphics_protocol() -> Option<&'static str> {
    match std::env::var("ALEXANDRIA_GRAPHICS").ok().as_deref() {
        Some("off") | Some("none") | Some("blocks") => return None,
        Some("kitty") => return Some("kitty"),
        Some("virtual") | Some("kitty-virtual") => return Some("kitty-virtual"),
        Some("iterm") => return Some("iterm"),
        _ => {}
    }
    if in_herdr() {
        return None;
    }
    let tp = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let term = std::env::var("TERM").unwrap_or_default();
    let kitty_family = std::env::var("KITTY_WINDOW_ID").is_ok()
        || term.contains("kitty")
        || tp == "ghostty"
        || std::env::var("GHOSTTY_RESOURCES_DIR").is_ok()
        || tp == "WezTerm";
    if kitty_family && kitty_supported() {
        Some("kitty")
    } else if tp == "iTerm.app" {
        Some("iterm")
    } else {
        None
    }
}

fn logo_cells(cols: usize) -> (usize, String) {
    let c = cols.saturating_sub(2).clamp(20, 120);
    let pad = " ".repeat(cols.saturating_sub(c) / 2);
    (c, pad)
}

fn kitty_logo(cols: usize) -> String {
    use base64::Engine;
    let (c, pad) = logo_cells(cols);
    let rows = ((c as f64 * 0.155 * 0.55).ceil() as usize).clamp(3, 12);
    let b64 = base64::engine::general_purpose::STANDARD.encode(LOGO_PNG);
    let bytes = b64.as_bytes();
    let chunks: Vec<&[u8]> = bytes.chunks(4096).collect();
    let mut out = String::new();
    out.push_str(&"\n".repeat(rows));
    out.push_str(&format!("\x1b[{rows}A"));
    out.push_str(&pad);
    for (i, chunk) in chunks.iter().enumerate() {
        let more = if i + 1 == chunks.len() { 0 } else { 1 };
        out.push_str("\x1b_G");
        if i == 0 {
            out.push_str(&format!("a=T,f=100,c={c},r={rows},C=1,q=2,"));
        }
        out.push_str(&format!("m={more};"));
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push_str("\x1b\\");
    }
    out.push_str(&format!("\r\x1b[{rows}B"));
    out
}

fn iterm_logo(cols: usize) -> String {
    use base64::Engine;
    let (c, pad) = logo_cells(cols);
    let b64 = base64::engine::general_purpose::STANDARD.encode(LOGO_PNG);
    format!("{pad}\x1b]1337;File=inline=1;width={c};preserveAspectRatio=1:{b64}\x07\n")
}

pub fn logo_banner(cols: usize) -> Option<String> {
    match graphics_protocol() {
        Some("kitty") => return Some(kitty_logo(cols)),
        Some("kitty-virtual") => return Some(kitty_virtual_logo(cols)),
        Some("iterm") => return Some(iterm_logo(cols)),
        _ => {}
    }
    halfblock_logo(cols)
}

fn halfblock_logo(cols: usize) -> Option<String> {
    let logo = load_logo().ok()?;
    let target_w = cols.saturating_sub(2).clamp(20, 110);
    let scale = target_w as f64 / logo.w as f64;
    let target_h = (((logo.h as f64 * scale) as usize).max(2) + 1) & !1;
    let pad = " ".repeat(cols.saturating_sub(target_w) / 2);
    let sample_px = |x: usize, y: usize| -> (u8, u8, u8, u8) {
        let x0 = ((x as f64 / scale) as usize).min(logo.w - 1);
        let y0 = ((y as f64 / scale) as usize).min(logo.h - 1);
        let x1 = (((x + 1) as f64 / scale).ceil() as usize).clamp(x0 + 1, logo.w);
        let y1 = (((y + 1) as f64 / scale).ceil() as usize).clamp(y0 + 1, logo.h);
        let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
        for sy in y0..y1 {
            for sx in x0..x1 {
                let o = (sy * logo.w + sx) * 4;
                let pa = logo.rgba[o + 3] as u32;
                r += logo.rgba[o] as u32 * pa / 255;
                g += logo.rgba[o + 1] as u32 * pa / 255;
                b += logo.rgba[o + 2] as u32 * pa / 255;
                a += pa;
                n += 1;
            }
        }
        ((r / n) as u8, (g / n) as u8, (b / n) as u8, (a / n) as u8)
    };
    let mut out = String::new();
    for row in 0..target_h / 2 {
        out.push_str(&pad);
        for x in 0..target_w {
            let (tr, tg, tb, ta) = sample_px(x, row * 2);
            let (br, bg, bb, ba) = if row * 2 + 1 < (logo.h as f64 * scale) as usize {
                sample_px(x, row * 2 + 1)
            } else {
                (0, 0, 0, 0)
            };
            match (ta > 12, ba > 12) {
                (false, false) => out.push_str("\x1b[0m "),
                (true, false) => {
                    out.push_str(&format!("\x1b[0m\x1b[38;2;{tr};{tg};{tb}m▀"));
                }
                (false, true) => {
                    out.push_str(&format!("\x1b[0m\x1b[38;2;{br};{bg};{bb}m▄"));
                }
                (true, true) => {
                    out.push_str(&format!(
                        "\x1b[38;2;{tr};{tg};{tb}m\x1b[48;2;{br};{bg};{bb}m▀"
                    ));
                }
            }
        }
        out.push_str("\x1b[0m\n");
    }
    Some(out)
}
