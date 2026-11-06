//! How a theme's colours become a whole palette.
//!
//! A theme file names a handful of colours — at least a card, a text and an accent — and
//! [`Ramp::palette`] maps them into the sixteen semantic slots the app paints with, **filling
//! every gap by derivation**: a theme that gives one background gets its gutter by darkening it,
//! one with a single text colour gets its second and third ranks by fading toward the card. The
//! themes that ship with the app (`crates/moosik/themes/`) are written the same way, listing
//! exactly what their upstream palettes publish.

use egui::Color32;

use super::Palette;

/// What a theme states. Six colours are required; the rest are filled in by
/// [`Ramp::palette`] when the family does not publish them, so nobody has to invent a hex.
#[derive(Clone, Copy)]
pub(super) struct Ramp {
    /// The card: the background most of the app is.
    pub(super) base: u32,
    /// The gutter behind the cards. Derived by darkening (dark) or lightening (light) `base`.
    pub(super) crust: Option<u32>,
    /// Menu and popup bodies. Derived as the step between `crust` and `base`.
    pub(super) mantle: Option<u32>,
    /// A raised surface inside a card: chips, inputs, row hovers at rest. Derived by stepping
    /// `base` toward the text where the family publishes only backgrounds.
    pub(super) surface: Option<u32>,
    /// Hovered and pressed surfaces. Derived by stepping `surface` toward the text colour.
    pub(super) surface_hi: Option<u32>,
    pub(super) surface_hi2: Option<u32>,
    /// Hairlines. Derived as a small step from `surface`.
    pub(super) outline: Option<u32>,
    pub(super) text: u32,
    /// Second-rank text. Derived by fading `text` toward `base`.
    pub(super) subtext: Option<u32>,
    /// Third-rank text. Derived by fading `text` further.
    pub(super) muted: Option<u32>,
    pub(super) accent: u32,
    /// The hover shade of the accent. Derived by lightening (dark) or darkening (light).
    pub(super) accent_hi: Option<u32>,
    /// What is legible *on* the accent — stated, never guessed; the contrast test checks it.
    pub(super) on_accent: u32,
    pub(super) red: u32,
    pub(super) yellow: u32,
}

/// Everything optional unset, so each table writes only what its family publishes.
pub(super) const RAMP: Ramp = Ramp {
    base: 0,
    crust: None,
    mantle: None,
    surface: None,
    surface_hi: None,
    surface_hi2: None,
    outline: None,
    text: 0,
    subtext: None,
    muted: None,
    accent: 0,
    accent_hi: None,
    on_accent: 0,
    red: 0,
    yellow: 0,
};

fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Ramp {
    pub(super) fn palette(&self, dark: bool) -> Palette {
        let published = rgb(self.base);
        let text = rgb(self.text);
        let surface = self
            .surface
            .map(rgb)
            .unwrap_or_else(|| published.lerp_to_gamma(text, 0.10));
        let accent = rgb(self.accent);
        // Darker than the card in *both* polarities — the gutter is a recess, not a highlight.
        // A light scheme lightened instead put Alucard's gutter in front of its own cards.
        // A big step down in the dark, a small one in the light, where the range is shallow.
        let base = published;
        let crust = self.crust.map(rgb).unwrap_or_else(|| {
            // 3% in the light: enough to read as a recess, shallow enough that a family whose
            // body text sits at the AA line (Solarized Light does, at 4.68:1 here) still clears
            // it on the gutter, where the now-playing title is painted.
            base.lerp_to_gamma(Color32::BLACK, if dark { 0.35 } else { 0.03 })
        });
        // Both tones are read on the cards and on the gutter (the player bar's song line and
        // times), so each has to clear its floor on whichever of the two is the weaker ground.
        let grounds = [base, crust];
        let secondary = lift_to_all(
            self.subtext
                .map(rgb)
                .unwrap_or_else(|| text.lerp_to_gamma(base, 0.22)),
            text,
            &grounds,
            SECONDARY_MIN_CONTRAST,
        );
        Palette {
            dark,
            window: crust,
            panel: base,
            surface,
            surface_hover: self
                .surface_hi
                .map(rgb)
                .unwrap_or_else(|| surface.lerp_to_gamma(text, 0.10)),
            surface_active: self
                .surface_hi2
                .map(rgb)
                .unwrap_or_else(|| surface.lerp_to_gamma(text, 0.18)),
            outline: self
                .outline
                .map(rgb)
                .unwrap_or_else(|| surface.lerp_to_gamma(text, 0.06)),
            text,
            secondary,
            dim: readable_dim(
                self.muted
                    .map(rgb)
                    .unwrap_or_else(|| text.lerp_to_gamma(base, 0.48)),
                text,
                secondary,
                &grounds,
            ),
            accent,
            accent_hover: self.accent_hi.map(rgb).unwrap_or_else(|| {
                if dark {
                    accent.lerp_to_gamma(Color32::WHITE, 0.18)
                } else {
                    accent.lerp_to_gamma(Color32::BLACK, 0.15)
                }
            }),
            on_accent: rgb(self.on_accent),
            danger: rgb(self.red),
            warning: rgb(self.yellow),
            overlay: self
                .mantle
                .map(rgb)
                .unwrap_or_else(|| crust.lerp_to_gamma(base, 0.5)),
            shadow: Color32::from_black_alpha(if dark { 150 } else { 45 }),
            // Opaque until `Palette::with_opacity` says otherwise.
            gutter_alpha: 1.0,
            card_alpha: 1.0,
            glass_alpha: 1.0,
            card_outline: true,
        }
    }
}

/// Relative luminance per WCAG, on straight (non-premultiplied) sRGB.
pub(crate) fn luminance(c: Color32) -> f32 {
    let ch = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
}

pub(crate) fn contrast(a: Color32, b: Color32) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    let (hi, lo) = if x > y { (x, y) } else { (y, x) };
    (hi + 0.05) / (lo + 0.05)
}

/// The least contrast the faintest text tone keeps against the card: counts, help lines, the
/// "Radio" tag. Several published families set theirs near 2:1 — a hint on their own
/// backgrounds, close to invisible under an acrylic shell — so this lifts it toward the text
/// colour until it reads, and leaves a family that already clears it alone. WCAG AA's 4.5:1 for
/// normal text: these are 11 pt, well under its large-text size.
pub(super) const DIM_MIN_CONTRAST: f32 = 4.5;

/// The least contrast second-rank text (subtitles, artist lines) keeps against the card: AA, with
/// room above [`DIM_MIN_CONTRAST`] so the faintest tone can clear AA and still sit under it.
pub(super) const SECONDARY_MIN_CONTRAST: f32 = 4.8;

/// The least contrast text painted in the accent (the playing song's title, the active lyric
/// line) keeps against the card: see [`Palette::accent_text`](crate::theme::Palette::accent_text).
pub(crate) const ACCENT_TEXT_MIN_CONTRAST: f32 = 4.5;

/// `colour`, pulled toward `toward` (the theme's own text colour, so the hue family holds) in small
/// steps until it clears `goal` on `panel`. A theme that already clears it is left alone, and the
/// pull stops at `toward` itself, which is the most the theme's own text can give.
pub(crate) fn lift_to(colour: Color32, toward: Color32, panel: Color32, goal: f32) -> Color32 {
    lift_to_all(colour, toward, &[panel], goal)
}

/// [`lift_to`], clearing `goal` on every one of `grounds`, each capped at what `toward` itself
/// reaches there.
pub(crate) fn lift_to_all(
    colour: Color32,
    toward: Color32,
    grounds: &[Color32],
    goal: f32,
) -> Color32 {
    let short = |c: Color32| {
        grounds
            .iter()
            .any(|&g| contrast(c, g) < goal.min(contrast(toward, g)))
    };
    let mut out = colour;
    let mut t = 0.0;
    while short(out) && t < 1.0 {
        t += 0.05;
        out = colour.lerp_to_gamma(toward, t);
    }
    out
}

/// `dim`, pulled toward `text` in small steps until it clears [`DIM_MIN_CONTRAST`] on `panel` —
/// or until it would crowd `secondary`, the tone above it, in a family whose own secondary
/// sits under that bar. The hierarchy outranks the number.
fn readable_dim(dim: Color32, text: Color32, secondary: Color32, grounds: &[Color32]) -> Color32 {
    let short = |c: Color32| {
        grounds
            .iter()
            .any(|&g| contrast(c, g) < DIM_MIN_CONTRAST.min(contrast(secondary, g) - 0.3))
    };
    let mut out = dim;
    let mut t = 0.0;
    while short(out) && t < 1.0 {
        t += 0.05;
        out = dim.lerp_to_gamma(text, t);
    }
    out
}
