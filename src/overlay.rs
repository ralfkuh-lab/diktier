//! Aufnahme-Overlay mit Mikrofonpegel (SPEC §4.5, `docs/overlay-plan.md`).
//!
//! Während `recording`, `transcribing` und `injecting` steht unten mittig auf
//! dem Monitor des fokussierten Fensters eine kleine dunkle Karte: Mikrofon-
//! Glyphe, scrollende Waveform-Historie und ein Pegelmeter mit Peak-Hold.
//!
//! **Fokusregel §4.2, nicht verhandelbar.** Das Fenster ist `WS_EX_NOACTIVATE`
//! + `WS_EX_TRANSPARENT`, wird mit `SW_SHOWNOACTIVATE` gezeigt, beantwortet
//! `WM_NCHITTEST` mit `HTTRANSPARENT` und ruft **nie** `SetForegroundWindow`
//! oder `SetFocus`. Tippen im Vordergrundfenster läuft beim Einblenden
//! ununterbrochen weiter, Klicks gehen durch die Karte hindurch.
//!
//! **Aufbau des Moduls.** Geometrie und Zeichnen sind reine Funktionen über
//! einem `&mut [u8]`-BGRA-Puffer — dadurch ohne Fenster testbar. Das Win32-
//! Fenster liegt darunter im Modul `windows` und wird ausschließlich auf
//! seinem Owner-Thread (`diktier-overlay`) erzeugt, bespielt und zerstört
//! (Phase-5-Leitentscheidung 2: kein fremder Thread fasst das `HWND` an, kein
//! `AttachThreadInput`).
//!
//! **Rendering-Vertrag.** Angezeigt wird ausschließlich über
//! `UpdateLayeredWindow` mit `AC_SRC_ALPHA`; der Puffer ist deshalb
//! **premultipliziertes** BGRA (B, G, R jeweils × A/255). Anders als beim
//! Tray-Icon (`tray::windows::make_icon`, Alpha nur 0 oder 255) braucht die
//! Karte echtes Zwischenalpha für die runden Ecken. DIB und Memory-DC leben
//! über Frames hinweg und werden nur bei Größenänderung neu gebaut.

use std::collections::VecDeque;
use std::time::Duration;

use crate::audio::level;

// ------------------------------------------------------------- Maße (96 dpi)

/// Referenzmaße der Karte bei 96 dpi; alles andere skaliert damit.
pub const CARD_W: i32 = 400;
pub const CARD_H: i32 = 72;
/// Abstand der Kartenunterkante zur Unterkante der Arbeitsfläche.
pub const CARD_MARGIN_BOTTOM: i32 = 72;
pub const CARD_RADIUS: i32 = 14;
/// Innenabstand der Karte.
pub const CARD_PADDING: i32 = 14;
/// Breite der Mikrofon-Glyphe.
pub const GLYPH_W: i32 = 20;
/// Abstand zwischen Glyphe und Waveform.
pub const GLYPH_GAP: i32 = 12;
/// Balken der Waveform: Breite und Lücke.
pub const BAR_W: i32 = 3;
pub const BAR_GAP: i32 = 2;
/// Höhe des Pegelmeters unter der Waveform.
pub const METER_H: i32 = 5;
/// Abstand zwischen Waveform und Meter.
pub const METER_GAP: i32 = 8;

// ------------------------------------------------------------------- Farben

/// Dunkle Karte mit leichter Transparenz (Optik nach Omarchy-Voxtype-OSD).
pub const CARD_COLOR: Color = Color::rgba(22, 22, 26, 235);
pub const GLYPH_COLOR: Color = Color::rgb(226, 228, 233);
/// Waveform beim Sprechen.
pub const WAVE_COLOR: Color = Color::rgb(88, 200, 160);
/// Waveform kurz vor Vollaussteuerung — dann ist das Mikrofon zu laut.
pub const WAVE_HOT_COLOR: Color = Color::rgb(233, 168, 78);
/// Grundlinie bei Stille: eine flache Linie, damit „Mikro stumm" sichtbar ist.
pub const WAVE_IDLE_COLOR: Color = Color::rgb(78, 84, 96);
pub const METER_TRACK_COLOR: Color = Color::rgba(255, 255, 255, 38);
pub const METER_FILL_COLOR: Color = Color::rgb(88, 200, 160);
pub const PEAK_COLOR: Color = Color::rgb(238, 240, 245);
/// Ab dieser Balkenhöhe gilt der Pegel als heiß.
const HOT_LEVEL: f32 = 0.85;

// ---------------------------------------------------------------- Geometrie

/// Rechteck in Pixeln, `right`/`bottom` exklusiv — dasselbe Verständnis wie
/// Win32-`RECT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub const fn width(&self) -> i32 {
        self.right - self.left
    }

    pub const fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub const fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }
}

/// Referenzmaß (96 dpi) → Pixel des Zielmonitors. Werte > 0 bleiben ≥ 1 px,
/// damit Linien bei kleiner Skalierung nicht verschwinden.
pub fn scale(value: i32, dpi: u32) -> i32 {
    if value == 0 {
        return 0;
    }
    let dpi = if dpi == 0 { 96 } else { dpi };
    let scaled = (f64::from(value) * f64::from(dpi) / 96.0).round() as i32;
    if value > 0 { scaled.max(1) } else { scaled.min(-1) }
}

/// Kartenrechteck in Bildschirmkoordinaten: unten mittig in der Arbeitsfläche
/// des Zielmonitors (Leitentscheidung 7).
///
/// `work` ist `MONITORINFO::rcWork` — also ohne Taskleiste. Passt die Karte
/// nicht (winzige Arbeitsfläche, riesige Skalierung), wird sie geklemmt statt
/// aus dem Monitor zu laufen.
pub fn card_rect(work: Rect, dpi: u32) -> Rect {
    let work_w = work.width().max(1);
    let work_h = work.height().max(1);
    let width = scale(CARD_W, dpi).min(work_w);
    let height = scale(CARD_H, dpi).min(work_h);
    let left = work.left + ((work_w - width) / 2).max(0);
    let mut top = work.bottom - scale(CARD_MARGIN_BOTTOM, dpi) - height;
    if top < work.top {
        // Kein Platz für den Bodenabstand: dann eben direkt über die Unterkante.
        top = (work.bottom - height).max(work.top);
    }
    Rect::new(left, top, left + width, top + height)
}

/// Wo innerhalb der Karte was liegt — in Puffer-Koordinaten (0/0 = links oben).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardLayout {
    pub card: Rect,
    pub glyph: Rect,
    pub wave: Rect,
    pub meter: Rect,
}

pub fn card_layout(width: i32, height: i32, dpi: u32) -> CardLayout {
    let card = Rect::new(0, 0, width.max(0), height.max(0));
    let pad = scale(CARD_PADDING, dpi);
    let glyph_w = scale(GLYPH_W, dpi);
    let glyph_h = (height - 2 * pad).max(0);
    let glyph = Rect::new(pad, pad, pad + glyph_w, pad + glyph_h);

    let content_left = glyph.right + scale(GLYPH_GAP, dpi);
    let content_right = width - pad;
    let meter_h = scale(METER_H, dpi);
    let meter = Rect::new(
        content_left,
        height - pad - meter_h,
        content_right,
        height - pad,
    );
    let wave = Rect::new(
        content_left,
        pad,
        content_right,
        meter.top - scale(METER_GAP, dpi),
    );
    CardLayout {
        card,
        glyph,
        wave,
        meter,
    }
}

/// Wie viele Balken passen nebeneinander in die Waveform-Fläche?
pub fn bar_capacity(area_width: i32, bar_w: i32, gap: i32) -> usize {
    if area_width <= 0 || bar_w <= 0 {
        return 0;
    }
    let pitch = bar_w + gap.max(0);
    if area_width < bar_w {
        return 0;
    }
    (((area_width - bar_w) / pitch) + 1).max(0) as usize
}

/// Rechteck des `index`-ten Balkens **von rechts** (0 = neuester Wert).
/// `None`, wenn er links aus der Fläche fiele.
///
/// `value` ist eine bereits normierte Balkenhöhe `0..1`; ein stiller Balken
/// bleibt eine 1 px hohe Linie, damit „Mikro stumm" sichtbar ist.
pub fn bar_rect(area: Rect, index: usize, value: f32, bar_w: i32, gap: i32) -> Option<Rect> {
    if bar_w <= 0 || area.height() <= 0 {
        return None;
    }
    let pitch = bar_w + gap.max(0);
    let right = area.right - (index as i32).checked_mul(pitch)?;
    let left = right - bar_w;
    if left < area.left {
        return None;
    }
    let full = ((value * area.height() as f32).round() as i32).clamp(1, area.height());
    let top = area.top + area.height() / 2 - full / 2;
    Some(Rect::new(left, top, right, top + full))
}

/// Testhilfe: dieselbe Geometrie wie [`draw_waveform`], nur als Liste.
///
/// Im Produktionspfad gibt es diesen `Vec` bewusst **nicht** — bei 50 fps
/// wäre er eine Allokation pro Frame (Sol-Impl-Review Minor 7).
#[cfg(test)]
fn bar_rects(area: Rect, history: &[f32], bar_w: i32, gap: i32) -> Vec<(Rect, f32)> {
    let capacity = bar_capacity(area.width(), bar_w, gap);
    let shown = history.len().min(capacity);
    let mut out = Vec::with_capacity(shown);
    for index in 0..shown {
        let value = level::normalize(history[history.len() - 1 - index]);
        match bar_rect(area, index, value, bar_w, gap) {
            Some(rect) => out.push((rect, value)),
            None => break,
        }
    }
    out
}

// ------------------------------------------------------------------ Zeichnen

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

/// Top-down 32-bpp-BGRA-Puffer mit **premultipliziertem** Alpha — genau das,
/// was `UpdateLayeredWindow` mit `AC_SRC_ALPHA` erwartet.
pub struct Canvas<'a> {
    pixels: &'a mut [u8],
    width: i32,
    height: i32,
}

impl<'a> Canvas<'a> {
    /// `None`, wenn der Puffer nicht mindestens `width * height * 4` Bytes hat.
    pub fn new(pixels: &'a mut [u8], width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        let needed = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
        if pixels.len() < needed {
            return None;
        }
        Some(Self {
            pixels,
            width,
            height,
        })
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    /// Alles durchsichtig — der Rand außerhalb der Karte bleibt so.
    pub fn clear(&mut self) {
        let used = (self.width as usize) * (self.height as usize) * 4;
        self.pixels[..used].fill(0);
    }

    /// Pixel als `[b, g, r, a]` — nur die Tests lesen den Puffer zurück.
    #[cfg(test)]
    pub fn pixel(&self, x: i32, y: i32) -> [u8; 4] {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return [0; 4];
        }
        let idx = self.index(x, y);
        [
            self.pixels[idx],
            self.pixels[idx + 1],
            self.pixels[idx + 2],
            self.pixels[idx + 3],
        ]
    }

    fn index(&self, x: i32, y: i32) -> usize {
        ((y as usize) * (self.width as usize) + (x as usize)) * 4
    }

    /// Ein Pixel über das schon Gezeichnete legen (`source-over`), mit
    /// `coverage` als Kantenglättung.
    ///
    /// Gerechnet wird premultipliziert: `dst = src + dst × (1 − a)`. Am Ende
    /// wird jeder Farbkanal auf das neue Alpha geklemmt — die Invariante
    /// `B, G, R ≤ A` darf auch durch Rundung nicht kippen, sonst zeigt
    /// `UpdateLayeredWindow` Ränder mit „mehr Farbe als Deckung".
    fn blend(&mut self, x: i32, y: i32, color: Color, coverage: f32) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let coverage = if coverage.is_finite() {
            coverage.clamp(0.0, 1.0)
        } else {
            return;
        };
        let alpha = f32::from(color.a) / 255.0 * coverage;
        if alpha <= 0.0 {
            return;
        }
        let inv = 1.0 - alpha;
        let idx = self.index(x, y);
        let dst_a = f32::from(self.pixels[idx + 3]);
        let new_a = (f32::from(u8::MAX) * alpha + dst_a * inv).round().min(255.0);
        for (offset, channel) in [color.b, color.g, color.r].into_iter().enumerate() {
            let src = f32::from(channel) * alpha;
            let dst = f32::from(self.pixels[idx + offset]);
            let value = (src + dst * inv).round().clamp(0.0, new_a);
            self.pixels[idx + offset] = value as u8;
        }
        self.pixels[idx + 3] = new_a as u8;
    }
}

pub fn fill_rect(canvas: &mut Canvas, rect: Rect, color: Color) {
    for y in rect.top.max(0)..rect.bottom.min(canvas.height()) {
        for x in rect.left.max(0)..rect.right.min(canvas.width()) {
            canvas.blend(x, y, color, 1.0);
        }
    }
}

/// Abgerundetes Rechteck mit weichen Kanten.
///
/// Die Deckung eines Pixels ergibt sich aus seinem Abstand zum nächsten Punkt
/// des inneren Rechtecks: innerhalb voll, im Übergangsband ein Zwischenwert,
/// außerhalb nichts. Genau daraus entsteht das Zwischenalpha, das die Ecken
/// sauber aussehen lässt.
pub fn fill_round_rect(canvas: &mut Canvas, rect: Rect, radius: i32, color: Color) {
    if rect.is_empty() {
        return;
    }
    let max_radius = (rect.width().min(rect.height()) / 2).max(0);
    let radius = radius.clamp(0, max_radius) as f32;
    let (inner_left, inner_right) = (rect.left as f32 + radius, rect.right as f32 - radius);
    let (inner_top, inner_bottom) = (rect.top as f32 + radius, rect.bottom as f32 - radius);

    for y in rect.top.max(0)..rect.bottom.min(canvas.height()) {
        for x in rect.left.max(0)..rect.right.min(canvas.width()) {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let cx = px.clamp(inner_left, inner_right);
            let cy = py.clamp(inner_top, inner_bottom);
            let (dx, dy) = (px - cx, py - cy);
            let distance = (dx * dx + dy * dy).sqrt();
            let coverage = if distance <= 0.0 {
                1.0
            } else {
                (radius + 0.5 - distance).clamp(0.0, 1.0)
            };
            canvas.blend(x, y, color, coverage);
        }
    }
}

/// Unterer Halbkreis als Strich — der Bügel unter der Mikrofonkapsel.
fn stroke_lower_arc(
    canvas: &mut Canvas,
    center_x: f32,
    center_y: f32,
    radius: f32,
    thickness: f32,
    color: Color,
) {
    let half = (thickness / 2.0).max(0.5);
    let outer = radius + half + 1.0;
    let x0 = (center_x - outer).floor() as i32;
    let x1 = (center_x + outer).ceil() as i32;
    let y0 = center_y.floor() as i32;
    let y1 = (center_y + outer).ceil() as i32;
    for y in y0..y1 {
        for x in x0..x1 {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            if py < center_y {
                continue;
            }
            let (dx, dy) = (px - center_x, py - center_y);
            let distance = (dx * dx + dy * dy).sqrt();
            let coverage = (half + 0.5 - (distance - radius).abs()).clamp(0.0, 1.0);
            canvas.blend(x, y, color, coverage);
        }
    }
}

/// Mikrofon aus Primitiven: Kapsel (abgerundetes Rechteck), Bügel (Bogen),
/// Ständer und Fuß. Kein GDI+, kein DirectWrite, keine Schrift.
pub fn draw_mic_glyph(canvas: &mut Canvas, area: Rect, color: Color) {
    if area.is_empty() {
        return;
    }
    let width = area.width() as f32;
    let height = area.height() as f32;
    let center_x = area.left as f32 + width / 2.0;

    let capsule_w = (width * 0.46).max(2.0);
    let capsule_h = (height * 0.52).max(3.0);
    let capsule = Rect::new(
        (center_x - capsule_w / 2.0).round() as i32,
        area.top,
        (center_x + capsule_w / 2.0).round() as i32,
        area.top + capsule_h.round() as i32,
    );
    fill_round_rect(canvas, capsule, (capsule_w / 2.0).round() as i32, color);

    let stroke = (width * 0.09).max(1.0);
    let arc_radius = (width * 0.36).max(2.0);
    let arc_center_y = capsule.bottom as f32 - capsule_w * 0.35;
    stroke_lower_arc(canvas, center_x, arc_center_y, arc_radius, stroke, color);

    let stand_top = (arc_center_y + arc_radius).round() as i32;
    let foot_h = stroke.round().max(1.0) as i32;
    let foot_top = area.bottom - foot_h;
    if foot_top > stand_top {
        let half = (stroke / 2.0).max(0.5);
        fill_rect(
            canvas,
            Rect::new(
                (center_x - half).round() as i32,
                stand_top,
                (center_x + half).round().max((center_x - half).round() + 1.0) as i32,
                foot_top,
            ),
            color,
        );
    }
    let foot_w = width * 0.5;
    fill_rect(
        canvas,
        Rect::new(
            (center_x - foot_w / 2.0).round() as i32,
            foot_top.max(area.top),
            (center_x + foot_w / 2.0).round() as i32,
            area.bottom,
        ),
        color,
    );
}

/// Waveform-Historie als vertikal zentrierte Balken, neueste rechts.
///
/// Läuft direkt über die `VecDeque` — kein `Vec` und keine Kopie pro Frame
/// (Sol-Impl-Review Minor 7).
pub fn draw_waveform(
    canvas: &mut Canvas,
    area: Rect,
    history: &VecDeque<f32>,
    bar_w: i32,
    gap: i32,
) {
    let capacity = bar_capacity(area.width(), bar_w, gap);
    for (index, value) in history.iter().rev().take(capacity).enumerate() {
        let value = level::normalize(*value);
        let Some(rect) = bar_rect(area, index, value, bar_w, gap) else {
            break;
        };
        let color = if value >= HOT_LEVEL {
            WAVE_HOT_COLOR
        } else if value <= 0.0 {
            WAVE_IDLE_COLOR
        } else {
            WAVE_COLOR
        };
        fill_rect(canvas, rect, color);
    }
}

/// Schmales Pegelmeter mit Peak-Hold-Marke. `level` und `peak` sind
/// Balkenhöhen `0..1` (also schon dB-abgebildet).
pub fn draw_meter(canvas: &mut Canvas, area: Rect, level: f32, peak: f32) {
    if area.is_empty() {
        return;
    }
    let radius = (area.height() / 2).max(0);
    fill_round_rect(canvas, area, radius, METER_TRACK_COLOR);

    let width = area.width() as f32;
    let level = level::normalize(level);
    let filled = (width * level).round() as i32;
    if filled > 0 {
        fill_round_rect(
            canvas,
            Rect::new(area.left, area.top, area.left + filled, area.bottom),
            radius,
            METER_FILL_COLOR,
        );
    }

    let peak = level::normalize(peak);
    if peak > 0.0 {
        let mark_w = (area.height() / 2).max(1);
        let x = area.left + (width * peak).round() as i32;
        let left = (x - mark_w).max(area.left);
        fill_rect(
            canvas,
            Rect::new(left, area.top, (left + mark_w).min(area.right), area.bottom),
            PEAK_COLOR,
        );
    }
}

// ------------------------------------------------------------------ Zustand

/// Was das Overlay zwischen zwei Frames mitnimmt: die Waveform-Historie und
/// die Peak-Hold-Marke. Die Historie entsteht **hier** — der Audio-Callback
/// kennt nur einen einzigen Pegelwert.
#[derive(Debug, Default)]
pub struct OverlayState {
    history: VecDeque<f32>,
    capacity: usize,
    peak: f32,
}

impl OverlayState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Wie viele Balken die Karte gerade fasst (ändert sich mit DPI/Größe).
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        self.trim();
    }

    /// Beim Ausblenden: die nächste Aufnahme fängt mit leerer Karte an.
    pub fn clear(&mut self) {
        self.history.clear();
        self.peak = 0.0;
    }

    /// Ein Frame: Pegel (linear, `0..1`) einhängen, Peak-Hold über die
    /// **gemessene** Zeit seit dem letzten Frame nachziehen.
    pub fn push(&mut self, level: f32, elapsed: Duration) {
        let height = level::bar_height(level);
        self.history.push_back(height);
        self.trim();
        self.peak = level::decay_peak(self.peak, elapsed).max(height);
    }

    fn trim(&mut self) {
        while self.history.len() > self.capacity {
            self.history.pop_front();
        }
    }

    /// Die Balkenhöhen, älteste zuerst — ohne Kopie.
    pub fn bars(&self) -> &VecDeque<f32> {
        &self.history
    }

    /// Testhilfe: Historie als `Vec` (älteste zuerst).
    #[cfg(test)]
    fn history(&self) -> Vec<f32> {
        self.history.iter().copied().collect()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.history.len()
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.history.is_empty()
    }

    /// Der zuletzt eingehängte Wert — er füllt das Meter.
    pub fn level(&self) -> f32 {
        self.history.back().copied().unwrap_or(0.0)
    }

    pub fn peak(&self) -> f32 {
        self.peak
    }
}

/// Ein kompletter Frame in den Puffer. Einzige Stelle, die weiß, wie das
/// Overlay aussieht — und rein, also ohne Fenster testbar.
pub fn draw_card(canvas: &mut Canvas, dpi: u32, state: &OverlayState) {
    canvas.clear();
    let layout = card_layout(canvas.width(), canvas.height(), dpi);
    fill_round_rect(canvas, layout.card, scale(CARD_RADIUS, dpi), CARD_COLOR);
    draw_mic_glyph(canvas, layout.glyph, GLYPH_COLOR);
    draw_waveform(
        canvas,
        layout.wave,
        state.bars(),
        scale(BAR_W, dpi),
        scale(BAR_GAP, dpi),
    );
    draw_meter(canvas, layout.meter, state.level(), state.peak());
}

/// Wie viele Balken die Karte bei dieser Größe/DPI fasst.
pub fn history_capacity(width: i32, height: i32, dpi: u32) -> usize {
    let layout = card_layout(width, height, dpi);
    bar_capacity(layout.wave.width(), scale(BAR_W, dpi), scale(BAR_GAP, dpi))
}

#[cfg(windows)]
pub mod windows;

#[cfg(windows)]
pub use windows::OverlayWindow;

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas_buffer(width: i32, height: i32) -> Vec<u8> {
        vec![0; (width as usize) * (height as usize) * 4]
    }

    /// Leitentscheidung 7: unten mittig in der Arbeitsfläche des Zielmonitors,
    /// Referenz 400×72 px mit 72 px Bodenabstand.
    #[test]
    fn the_card_sits_bottom_center_of_the_work_area() {
        // Primärmonitor 1920×1080, Taskleiste unten (40 px).
        let work = Rect::new(0, 0, 1920, 1040);
        let card = card_rect(work, 96);
        assert_eq!(card.width(), CARD_W);
        assert_eq!(card.height(), CARD_H);
        assert_eq!(card.left, (1920 - CARD_W) / 2);
        assert_eq!(card.bottom, 1040 - CARD_MARGIN_BOTTOM);
        assert_eq!(
            card.left - work.left,
            work.right - card.right,
            "gleiche Ränder links und rechts"
        );
    }

    /// Zweitmonitor: dieselbe Rechnung, nur mit dem Ursprung des Monitors —
    /// die Karte folgt dem fokussierten Fenster, nicht dem Primärmonitor.
    #[test]
    fn a_second_monitor_gets_the_card_in_its_own_work_area() {
        let work = Rect::new(1920, -120, 4480, 1320);
        let card = card_rect(work, 96);
        assert!(card.left >= work.left && card.right <= work.right);
        assert_eq!(card.left, 1920 + (2560 - CARD_W) / 2);
        assert_eq!(card.bottom, 1320 - CARD_MARGIN_BOTTOM);

        // Negative Koordinaten (Monitor links/oberhalb des Primären).
        let left_of_primary = card_rect(Rect::new(-1920, -1080, 0, -40), 96);
        assert_eq!(left_of_primary.left, -1920 + (1920 - CARD_W) / 2);
        assert_eq!(left_of_primary.bottom, -40 - CARD_MARGIN_BOTTOM);
    }

    /// 150 % (144 dpi): alles skaliert mit, die Karte bleibt mittig.
    #[test]
    fn the_card_scales_with_the_monitor_dpi() {
        let work = Rect::new(0, 0, 1920, 1040);
        let card = card_rect(work, 144);
        assert_eq!(card.width(), 600);
        assert_eq!(card.height(), 108);
        assert_eq!(card.left, (1920 - 600) / 2);
        assert_eq!(card.bottom, 1040 - 108);

        // 200 % und 125 % ebenfalls.
        assert_eq!(card_rect(work, 192).width(), 800);
        assert_eq!(card_rect(work, 120).width(), 500);
        assert_eq!(scale(BAR_W, 144), 5, "3 px @96 → 4,5 → 5 px @144");
        assert_eq!(scale(0, 144), 0);
        assert_eq!(scale(1, 48), 1, "dünne Linien verschwinden nie ganz");
    }

    /// Winzige Arbeitsfläche: geklemmt statt aus dem Monitor gelaufen.
    #[test]
    fn the_card_is_clamped_into_a_tiny_work_area() {
        let work = Rect::new(0, 0, 320, 100);
        let card = card_rect(work, 96);
        assert!(card.left >= work.left && card.right <= work.right, "{card:?}");
        assert!(card.top >= work.top && card.bottom <= work.bottom, "{card:?}");
    }

    /// Historie kürzer als die Karte: links bleibt Platz, der neueste Balken
    /// steht rechts.
    #[test]
    fn a_short_history_is_right_aligned() {
        let area = Rect::new(0, 0, 100, 40);
        let bars = bar_rects(area, &[0.2, 0.4, 1.0], 3, 2);
        assert_eq!(bars.len(), 3);
        assert_eq!(bars[0].0.right, area.right, "neuester Balken ganz rechts");
        assert!((bars[0].1 - 1.0).abs() < 1e-6, "und trägt den neuesten Wert");
        assert_eq!(bars[1].0.right, area.right - 5);
        assert!(bars[2].0.left > area.left, "links bleibt Platz");
        // Vollausschlag füllt die Fläche, Stille bleibt eine 1-px-Linie.
        assert_eq!(bars[0].0.height(), area.height());
        let silent = bar_rects(area, &[0.0], 3, 2);
        assert_eq!(silent[0].0.height(), 1, "Stille = flache Linie");
        assert_eq!(
            silent[0].0.top,
            area.top + area.height() / 2,
            "vertikal zentriert"
        );
    }

    /// Historie länger als die Karte: die ältesten Werte fallen weg, es wird
    /// nie über den Rand hinaus gezeichnet.
    #[test]
    fn a_long_history_is_cut_at_the_left_edge() {
        let area = Rect::new(10, 0, 110, 40);
        let capacity = bar_capacity(area.width(), 3, 2);
        assert_eq!(capacity, 20);
        let history: Vec<f32> = (0..500).map(|i| (i % 10) as f32 / 10.0).collect();
        let bars = bar_rects(area, &history, 3, 2);
        assert_eq!(bars.len(), capacity);
        for (rect, _) in &bars {
            assert!(rect.left >= area.left, "{rect:?} läuft links heraus");
            assert!(rect.right <= area.right, "{rect:?} läuft rechts heraus");
        }
        assert!(
            (bars[0].1 - history[history.len() - 1]).abs() < 1e-6,
            "rechts steht der neueste Wert"
        );

        // Entartete Flächen liefern gar nichts, statt zu panicken.
        assert!(bar_rects(Rect::new(0, 0, 0, 40), &history, 3, 2).is_empty());
        assert!(bar_rects(Rect::new(0, 0, 100, 0), &history, 3, 2).is_empty());
        assert_eq!(bar_capacity(2, 3, 2), 0);
    }

    /// Die Kartenmaske: Ecke durchsichtig, Kante deckend — und **jedes** Pixel
    /// erfüllt die Premultiplikations-Invariante B, G, R ≤ A. Ohne sie zeigt
    /// `UpdateLayeredWindow` helle Säume an den Rundungen.
    #[test]
    fn the_card_mask_has_soft_corners_and_stays_premultiplied() {
        let (w, h) = (CARD_W, CARD_H);
        let mut buffer = canvas_buffer(w, h);
        let mut canvas = Canvas::new(&mut buffer, w, h).expect("Puffer passt");
        let state = OverlayState::new();
        draw_card(&mut canvas, 96, &state);

        // Äußerste Ecken: außerhalb der Rundung, also durchsichtig.
        for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            assert_eq!(canvas.pixel(x, y)[3], 0, "Ecke {x}/{y} ist nicht frei");
        }
        // Kantenmitten: volle Deckung der Karte.
        for (x, y) in [(w / 2, 0), (w / 2, h - 1), (0, h / 2), (w - 1, h / 2)] {
            assert_eq!(
                canvas.pixel(x, y)[3],
                CARD_COLOR.a,
                "Kante {x}/{y} ist nicht deckend"
            );
        }
        // Irgendwo im Übergangsband muss echtes Zwischenalpha stehen — genau
        // das kann das Tray-Icon (nur 0/255) nicht.
        let radius = scale(CARD_RADIUS, 96);
        let mut soft = 0;
        for y in 0..radius {
            for x in 0..radius {
                let alpha = canvas.pixel(x, y)[3];
                if alpha > 0 && alpha < CARD_COLOR.a {
                    soft += 1;
                }
            }
        }
        assert!(soft > 0, "keine geglättete Kante gefunden");

        for y in 0..h {
            for x in 0..w {
                let [b, g, r, a] = canvas.pixel(x, y);
                assert!(
                    b <= a && g <= a && r <= a,
                    "Pixel {x}/{y} ist nicht premultipliziert: {b}/{g}/{r} über {a}"
                );
            }
        }
    }

    /// Auch mit voller Waveform und Peak-Marke bleibt alles premultipliziert
    /// und im Puffer — bei 96 wie bei 144 dpi.
    #[test]
    fn a_full_frame_stays_inside_the_buffer_and_premultiplied() {
        for dpi in [96, 120, 144, 192] {
            let card = card_rect(Rect::new(0, 0, 1920, 1040), dpi);
            let (w, h) = (card.width(), card.height());
            let mut buffer = canvas_buffer(w, h);
            let mut canvas = Canvas::new(&mut buffer, w, h).expect("Puffer passt");
            let mut state = OverlayState::new();
            state.set_capacity(history_capacity(w, h, dpi));
            for step in 0..500 {
                state.push(
                    (step % 20) as f32 / 20.0,
                    Duration::from_millis(20),
                );
            }
            draw_card(&mut canvas, dpi, &state);
            for y in 0..h {
                for x in 0..w {
                    let [b, g, r, a] = canvas.pixel(x, y);
                    assert!(b <= a && g <= a && r <= a, "dpi {dpi}, Pixel {x}/{y}");
                }
            }
            assert!(
                state.len() <= history_capacity(w, h, dpi),
                "dpi {dpi}: Historie wächst unbegrenzt"
            );
        }
    }

    /// Die Historie ist ein Fenster fester Breite: neue Werte rechts hinein,
    /// alte links hinaus. `Hide` leert sie.
    #[test]
    fn the_history_is_a_sliding_window_that_hide_empties() {
        let mut state = OverlayState::new();
        state.set_capacity(4);
        for value in [0.1, 0.2, 0.3, 0.4, 0.5, 0.6] {
            state.push(value, Duration::from_millis(20));
        }
        assert_eq!(state.len(), 4);
        let history = state.history();
        assert!(
            (history[3] - level::bar_height(0.6)).abs() < 1e-6,
            "rechts steht der neueste Wert"
        );
        assert!((state.level() - level::bar_height(0.6)).abs() < 1e-6);

        // Kleinere Karte (DPI-Wechsel): die Historie schrumpft sofort mit.
        state.set_capacity(2);
        assert_eq!(state.len(), 2);

        state.clear();
        assert!(state.is_empty());
        assert_eq!(state.peak(), 0.0);
        assert_eq!(state.level(), 0.0);
    }

    /// Nach dem Loslassen kommt kein Pegel mehr: die Waveform läuft leer, die
    /// Peak-Marke fällt (Leitentscheidung 5 und 8).
    #[test]
    fn without_new_level_the_waveform_runs_empty_and_the_peak_falls() {
        let mut state = OverlayState::new();
        state.set_capacity(8);
        state.push(1.0, Duration::from_millis(20));
        assert!((state.peak() - 1.0).abs() < 1e-6);

        for _ in 0..8 {
            state.push(0.0, Duration::from_millis(20));
        }
        assert!(
            state.history().iter().all(|v| *v == 0.0),
            "die Karte muss leerlaufen"
        );
        assert!(state.peak() < 1.0, "die Marke muss fallen");
        for _ in 0..200 {
            state.push(0.0, Duration::from_millis(20));
        }
        assert_eq!(state.peak(), 0.0);
    }

    /// Der Canvas nimmt nur Puffer, die wirklich groß genug sind — sonst
    /// schriebe das Zeichnen über den DIB hinaus.
    #[test]
    fn a_canvas_rejects_buffers_that_are_too_small() {
        let mut small = vec![0u8; 10];
        assert!(Canvas::new(&mut small, 40, 20).is_none());
        let mut zero: Vec<u8> = Vec::new();
        assert!(Canvas::new(&mut zero, 0, 0).is_none());
        let mut ok = canvas_buffer(4, 4);
        assert!(Canvas::new(&mut ok, 4, 4).is_some());
    }

    /// Zeichnen darf nie über den Rand hinauslaufen — auch nicht mit
    /// Rechtecken, die außerhalb liegen.
    #[test]
    fn drawing_outside_the_canvas_is_clipped() {
        let mut buffer = canvas_buffer(8, 8);
        let mut canvas = Canvas::new(&mut buffer, 8, 8).expect("Puffer passt");
        fill_rect(&mut canvas, Rect::new(-20, -20, 40, 40), Color::rgb(255, 0, 0));
        fill_round_rect(
            &mut canvas,
            Rect::new(-5, -5, 100, 100),
            4,
            Color::rgba(0, 255, 0, 128),
        );
        draw_mic_glyph(&mut canvas, Rect::new(-10, -10, 4, 4), GLYPH_COLOR);
        draw_meter(&mut canvas, Rect::new(4, 4, 200, 6), 1.0, 1.0);
        // Kein Panic, und die Invariante hält weiterhin.
        for y in 0..8 {
            for x in 0..8 {
                let [b, g, r, a] = canvas.pixel(x, y);
                assert!(b <= a && g <= a && r <= a, "Pixel {x}/{y}");
            }
        }
    }

    /// `draw_waveform` läuft über die `VecDeque`, `bar_rects` über einen
    /// Slice — beide müssen dieselbe Reihenfolge malen. Der Test prüft das am
    /// Bild: der jüngste (laute) Wert steht ganz rechts, davor liegt Stille
    /// als 1-px-Linie (Sol-Impl-Review Minor 7).
    #[test]
    fn the_newest_bar_is_painted_at_the_right_edge() {
        let (w, h) = (40, 20);
        let mut buffer = canvas_buffer(w, h);
        let mut canvas = Canvas::new(&mut buffer, w, h).expect("Puffer passt");
        let area = Rect::new(0, 0, w, h);

        let mut history: VecDeque<f32> = VecDeque::new();
        for _ in 0..5 {
            history.push_back(0.0);
        }
        history.push_back(1.0);
        draw_waveform(&mut canvas, area, &history, 3, 2);

        // Rechteste Balkenspalte: über die volle Höhe gemalt.
        assert!(canvas.pixel(w - 1, 0)[3] > 0, "oberste Zeile fehlt");
        assert!(canvas.pixel(w - 1, h - 1)[3] > 0, "unterste Zeile fehlt");
        // Der Balken davor ist Stille — nur die Mittellinie.
        let previous_x = w - 1 - 5;
        assert_eq!(canvas.pixel(previous_x, 0)[3], 0, "Stille malt keine Säule");
        assert!(canvas.pixel(previous_x, h / 2)[3] > 0, "Grundlinie fehlt");

        // Und dieselbe Geometrie wie die Testhilfe.
        let slice: Vec<f32> = history.iter().copied().collect();
        let rects = bar_rects(area, &slice, 3, 2);
        assert_eq!(rects[0].0.right, w);
        assert_eq!(rects[0].0.height(), h);
        assert_eq!(rects[1].0.height(), 1);
    }

    /// Das Meter füllt sich mit dem Pegel und trägt die Peak-Marke rechts
    /// davon.
    #[test]
    fn the_meter_fills_with_the_level_and_marks_the_peak() {
        let (w, h) = (60, 10);
        let mut buffer = canvas_buffer(w, h);
        let mut canvas = Canvas::new(&mut buffer, w, h).expect("Puffer passt");
        let area = Rect::new(0, 2, w, 8);
        draw_meter(&mut canvas, area, 0.25, 0.75);

        let y = 5;
        let filled = canvas.pixel(5, y);
        assert!(filled[1] > 100, "linkes Viertel ist gefüllt: {filled:?}");
        let empty = canvas.pixel(40, y);
        assert!(
            empty[1] < filled[1],
            "hinter dem Pegel ist nur die Spur: {empty:?}"
        );
        let mark = canvas.pixel((w as f32 * 0.75) as i32 - 1, y);
        assert!(mark[3] > 200 && mark[0] > 200, "Peak-Marke fehlt: {mark:?}");
    }
}

