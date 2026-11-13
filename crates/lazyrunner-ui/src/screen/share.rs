//! An app shared through a tunnel (#102), in its title: a spinner while the tool connects, then its public host - a
//! shimmer runs along it as it's copied - and how many requests came since it was shared. `Alt+q` shows the URL as a
//! QR code, for a phone; `y` there copies it again.

use std::time::Duration;

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::controller::Tunnel;
use qrcode::{Color, QrCode};

use super::icons::{self, Icon};
use super::overlays::boxed;
use super::seg::{Seg, on, seg};
use super::theme::C;
use super::{Overlay, Ui};

/// The shimmer's run along the host as it's copied, and the light border a QR code reader needs, in modules.
const SHIMMER: Duration = Duration::from_millis(900);
const QUIET: usize = 2;
const FOOTER: &str = "point a phone's camera at it · y copies the URL · Esc closes";

/// The focused app's tunnel and its URL, once it came.
fn shared(ui: &Ui, controller: &Controller) -> Result<(usize, String), String> {
    let app = controller.apps.get(ui.focus).ok_or_else(String::new)?;
    let tunnel = controller.tunnel(&app.key).ok_or_else(|| format!("{} isn't shared - Alt+s shares it.", app.name))?;
    let url = tunnel.url.clone().ok_or_else(|| format!("{}'s tunnel is still connecting.", app.name))?;
    Ok((ui.focus, url))
}

/// Alt+q: the focused app's public URL as a QR code.
pub fn open(ui: &mut Ui, controller: &mut Controller) {
    match shared(ui, controller) {
        Ok((app, _)) => ui.overlay = Some(Overlay::ShareQr { app }),
        Err(why) if why.is_empty() => {}
        Err(why) => controller.add_log(why, Hue::DarkGray),
    }
}

pub fn key(ui: &mut Ui, controller: &mut Controller, name: &str) {
    match name {
        "Escape" | "q" | "Alt+q" => ui.overlay = None,
        "y" => {
            if let Ok((_, url)) = shared(ui, controller) {
                super::copy::put(ui.clipboard, controller, &url, "the public URL");
            }
        }
        _ => {}
    }
}

/// The code as half blocks, two modules a row: light modules drawn, dark ones left black, with a light border.
fn drawn(url: &str) -> Option<Vec<String>> {
    let code = QrCode::new(url.as_bytes()).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let size = width + QUIET * 2;
    let light = |x: usize, y: usize| {
        let inside = (QUIET..QUIET + width).contains(&x) && (QUIET..QUIET + width).contains(&y);
        y >= size || !inside || colors[(y - QUIET) * width + (x - QUIET)] == Color::Light
    };
    let rows = (0..size)
        .step_by(2)
        .map(|y| {
            (0..size)
                .map(|x| match (light(x, y), light(x, y + 1)) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                })
                .collect()
        })
        .collect();
    Some(rows)
}

pub fn rows(ui: &Ui, controller: &Controller, cells: usize, rows: usize) -> Vec<Vec<Seg>> {
    let Ok((app, url)) = shared(ui, controller) else { return Vec::new() };
    let name = controller.apps.get(app).map_or("", |app| app.name.as_str());
    let code = drawn(&url).unwrap_or_default();
    let inner = code.first().map_or(0, |row| row.chars().count()).max(url.chars().count()) + 4;
    let mut lines: Vec<Vec<Seg>> = code
        .into_iter()
        .map(|row| {
            let gap = inner.saturating_sub(row.chars().count()) / 2;
            vec![seg(" ".repeat(gap), C::Gray), on(row, C::White, C::Black)]
        })
        .collect();
    let gap = inner.saturating_sub(url.chars().count()) / 2;
    lines.extend([Vec::new(), vec![seg(" ".repeat(gap), C::Gray), seg(url, C::Cyan)]]);
    let title = icons::titled(ui.nerd, Icon::Link, &format!("{name}, shared"));
    boxed(&title, lines, FOOTER, cells, rows, inner.min(cells.saturating_sub(4)))
}

/// The title's chip for a shared app: `⇄ ⠋ connecting`, then `⇄ host · 12 requests` - the host shimmering as it's copied.
pub fn chip(ui: &Ui, controller: &Controller, key: &str, tunnel: &Tunnel) -> Vec<Seg> {
    let Some(url) = tunnel.url.as_deref() else {
        let spin = if ui.reduced_motion { "…" } else { super::columns::spinner(ui) };
        return vec![seg(format!(" ⇄ {spin} connecting"), C::Cyan)];
    };
    let host = url.trim_start_matches("https://");
    let count = controller
        .requests
        .items
        .iter()
        .filter(|request| request.app == key && request.ended.is_some_and(|at| at >= tunnel.since))
        .count();
    let mut segs = vec![seg(" ⇄ ", C::Cyan)];
    let since =
        ui.tunnel_copied.get(key).map(|at| ui.now.saturating_duration_since(*at)).filter(|since| *since < SHIMMER);
    match since.filter(|_| !ui.reduced_motion) {
        None => segs.push(seg(host.to_string(), C::Cyan)),
        Some(since) => {
            let length = host.chars().count();
            let head = since.as_secs_f64() / SHIMMER.as_secs_f64() * (length + 4) as f64 - 2.0;
            segs.extend(host.chars().enumerate().map(|(at, letter)| {
                let away = (at as f64 - head).abs();
                let text = letter.to_string();
                if away < 1.0 {
                    on(text, C::Black, C::White)
                } else if away < 2.5 {
                    seg(text, C::White)
                } else {
                    seg(text, C::Cyan)
                }
            }));
        }
    }
    if count > 0 {
        segs.push(seg(format!(" · {count} request{}", if count == 1 { "" } else { "s" }), C::DarkCyan));
    }
    segs
}

/// Whether a host shimmers or a tunnel connects.
pub fn animating(ui: &Ui, controller: &Controller) -> bool {
    let shimmering = ui.tunnel_copied.values().any(|at| ui.now.saturating_duration_since(*at) < SHIMMER);
    let connecting = controller.apps.iter().any(|app| controller.tunnel(&app.key).is_some_and(|it| it.url.is_none()));
    !ui.reduced_motion && (shimmering || connecting)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_draws_as_a_square_code_two_modules_a_row_with_a_light_border() {
        let rows = drawn("https://abc123.devtunnels.ms").expect("a code");
        let width = rows[0].chars().count();
        assert!(rows.iter().all(|row| row.chars().count() == width));
        assert_eq!(rows.len(), width.div_ceil(2), "two modules a row");
        assert!(rows[0].chars().all(|cell| cell == '█'), "the border is light");
    }
}
