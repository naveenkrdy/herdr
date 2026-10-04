//! The board's look: every colour comes from the active theme's palette roles. A fill is a
//! role mixed into the ground the board draws on, so no theme needs colours of its own; a
//! theme without RGB colours gets a surface role instead.

use ratatui::style::{Color, Modifier, Style};

use super::model::{TaskGroup, TaskSection, WorkKind, WorkStage};
use crate::app::state::Palette;

/// What the board draws on: the panel background, or the darkest surface when the theme
/// leaves the panel to the terminal.
fn ground(palette: &Palette) -> Color {
    match palette.panel_bg {
        Color::Reset => palette.surface_dim,
        color => color,
    }
}

/// `color` mixed into the ground by `amount` (0 is the ground, 1 is `color`); `fallback`
/// when either is not an RGB colour.
fn tint(color: Color, amount: f32, palette: &Palette, fallback: Color) -> Color {
    let (Color::Rgb(r, g, b), Color::Rgb(base_r, base_g, base_b)) = (color, ground(palette)) else {
        return fallback;
    };
    let mix = |base: u8, top: u8| {
        let mixed = f32::from(base) + (f32::from(top) - f32::from(base)) * amount;
        mixed.round().clamp(0.0, 255.0) as u8
    };
    Color::Rgb(mix(base_r, r), mix(base_g, g), mix(base_b, b))
}

/// The header's band, across the whole row.
pub(super) fn band(palette: &Palette) -> Style {
    Style::default().bg(tint(palette.text, 0.06, palette, palette.surface0))
}

/// A button: the space picker, the close button, an inactive tab's fill.
pub(super) fn button(palette: &Palette) -> Style {
    Style::default()
        .fg(palette.text)
        .bg(tint(palette.text, 0.14, palette, palette.surface1))
}

/// A key a click or a press does: a button, plain even on a bold cell. The dialogs draw
/// their keys with it too.
pub(in crate::client::shell) fn key(palette: &Palette) -> Style {
    button(palette).remove_modifier(Modifier::BOLD)
}

/// What is chosen: the view tab that shows, a menu's chosen item. Text on the selection fill,
/// the one fill a selected row takes too.
pub(super) fn chosen(palette: &Palette) -> Style {
    Style::default().fg(palette.text).bg(palette.selection_bg)
}

/// The view tab that shows, on the selection fill, and the one that does not, grey on no
/// fill: a button's fill would outshine the selection fill on a theme where it is lighter.
pub(super) fn tab(palette: &Palette, on: bool) -> Style {
    if on {
        chosen(palette)
    } else {
        Style::default().fg(palette.overlay1)
    }
}

/// The filter box: dim at rest, text on a tint of the accent while it narrows the board.
pub(super) fn filter_box(palette: &Palette, active: bool) -> Style {
    if active {
        Style::default()
            .fg(palette.text)
            .bg(tint(palette.accent, 0.18, palette, palette.surface1))
    } else {
        Style::default().fg(palette.overlay1).bg(tint(
            palette.text,
            0.10,
            palette,
            palette.surface0,
        ))
    }
}

/// A section's or a stage's title: its colour on a fill of it.
pub(in crate::client::shell) fn label(role: Color, palette: &Palette) -> Style {
    Style::default()
        .fg(role)
        .bg(tint(role, 0.18, palette, palette.surface0))
        .add_modifier(Modifier::BOLD)
}

/// A field's or a block's name in the details panel: grey, so it reads apart from its value
/// in text, as a column's name does.
pub(super) fn field_label(palette: &Palette) -> Style {
    Style::default().fg(palette.overlay1)
}

/// A section's box and the lines inside it: its colour, quieted into the ground so the
/// heading and the tasks stand out; the colour itself without RGB.
pub(super) fn frame(role: Color, palette: &Palette) -> Style {
    Style::default().fg(tint(role, 0.45, palette, role))
}

/// A chip's mark, and its words and count: the mark in its colour, the words grey on no fill;
/// the one the view shows alone on a tint of its colour, its words text. None bold.
pub(super) fn chip(role: Color, on: bool, palette: &Palette) -> (Style, Style) {
    let ground = if on {
        Style::default().bg(tint(role, 0.18, palette, palette.surface0))
    } else {
        Style::default()
    };
    let words = if on { palette.text } else { palette.overlay1 };
    (ground.fg(role), ground.fg(words))
}

/// A word the filter matched.
pub(super) fn mark(palette: &Palette) -> Style {
    Style::default()
        .fg(palette.text)
        .bg(tint(palette.accent, 0.35, palette, palette.surface1))
}

/// The lilac the board draws a lilac role in when the theme's mauve is not one: light on a
/// dark ground, deep on a light one.
const LILAC_ON_DARK: Color = Color::Rgb(196, 167, 231);
const LILAC_ON_LIGHT: Color = Color::Rgb(124, 77, 196);

/// Hues that read as violet through magenta and pink, in degrees.
const LILAC_HUES: std::ops::RangeInclusive<f32> = 240.0..=350.0;

/// A mauve nearer the yellow than this (the squared RGB distance) reads as it.
const NEAR_YELLOW: i32 = 50 * 50;

/// The board's lilac (WAITING ON PEOPLE): the theme's mauve when it is a
/// violet, magenta or pink apart from its yellow, else a lilac of the board's own. A mauve
/// the terminal colours stays, as the rest of such a theme does.
pub(in crate::client::shell) fn lilac(palette: &Palette) -> Color {
    let Color::Rgb(r, g, b) = palette.mauve else {
        return palette.mauve;
    };
    let near_yellow = match palette.yellow {
        Color::Rgb(yr, yg, yb) => {
            let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
            d(r, yr) + d(g, yg) + d(b, yb) < NEAR_YELLOW
        }
        _ => false,
    };
    if LILAC_HUES.contains(&hue(r, g, b)) && !near_yellow {
        return palette.mauve;
    }
    match ground(palette) {
        Color::Rgb(r, g, b) if luma(r, g, b) > 140.0 => LILAC_ON_LIGHT,
        _ => LILAC_ON_DARK,
    }
}

/// REVIEW's rose: the lilac's lightness at a pink hue, so it reads beside PLAN's lilac as
/// its kin and apart from BUILD's yellow and SHIP's green. A lilac the terminal colours
/// gives the terminal's magenta.
pub(in crate::client::shell) fn rose(palette: &Palette) -> Color {
    let Color::Rgb(r, g, b) = lilac(palette) else {
        return Color::Magenta;
    };
    let (red, green, blue) = from_lch(lightness(r, g, b), ROSE_CHROMA, ROSE_HUE);
    Color::Rgb(red, green, blue)
}

/// The rose's chroma and hue in CIE LCh.
const ROSE_CHROMA: f32 = 45.0;
const ROSE_HUE: f32 = 15.0;

/// The D65 white point in CIE XYZ.
const WHITE: (f32, f32, f32) = (0.950_47, 1.0, 1.088_83);

/// An sRGB channel as linear light, 0 up to 1.
fn linear(channel: u8) -> f32 {
    let value = f32::from(channel) / 255.0;
    if value > 0.040_45 {
        ((value + 0.055) / 1.055).powf(2.4)
    } else {
        value / 12.92
    }
}

/// Linear light as an sRGB channel, clamped to 0 up to 255.
fn encoded(value: f32) -> u8 {
    let value = if value <= 0.003_130_8 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    // Clamped to 0..=255 first, so the cast keeps the value.
    (255.0 * value).round().clamp(0.0, 255.0) as u8
}

/// An RGB colour's CIE L* lightness, 0 up to 100.
fn lightness(r: u8, g: u8, b: u8) -> f32 {
    let y = 0.212_672_9 * linear(r) + 0.715_152_2 * linear(g) + 0.072_175 * linear(b);
    let f = if y > 216.0 / 24_389.0 {
        y.cbrt()
    } else {
        (24_389.0 / 27.0 * y + 16.0) / 116.0
    };
    116.0 * f - 16.0
}

/// A CIE LCh colour (hue in degrees) as sRGB, each channel clamped.
fn from_lch(lightness: f32, chroma: f32, hue: f32) -> (u8, u8, u8) {
    let (a, b) = (
        chroma * hue.to_radians().cos(),
        chroma * hue.to_radians().sin(),
    );
    let fy = (lightness + 16.0) / 116.0;
    let (fx, fz) = (fy + a / 500.0, fy - b / 200.0);
    let back = |t: f32| {
        if t.powi(3) > 216.0 / 24_389.0 {
            t.powi(3)
        } else {
            (116.0 * t - 16.0) * 27.0 / 24_389.0
        }
    };
    let (x, y, z) = (WHITE.0 * back(fx), WHITE.1 * back(fy), WHITE.2 * back(fz));
    (
        encoded(3.240_454_2 * x - 1.537_138_5 * y - 0.498_531_4 * z),
        encoded(-0.969_266 * x + 1.876_010_8 * y + 0.041_556 * z),
        encoded(0.055_643_4 * x - 0.204_025_9 * y + 1.057_225_2 * z),
    )
}

/// An RGB colour's hue in degrees, 0 up to 360; 0 for a grey.
fn hue(r: u8, g: u8, b: u8) -> f32 {
    let (r, g, b) = (f32::from(r), f32::from(g), f32::from(b));
    let max = r.max(g).max(b);
    let span = max - r.min(g).min(b);
    if span == 0.0 {
        return 0.0;
    }
    let sixths = if max == r {
        ((g - b) / span).rem_euclid(6.0)
    } else if max == g {
        (b - r) / span + 2.0
    } else {
        (r - g) / span + 4.0
    };
    sixths * 60.0
}

/// How light an RGB colour looks, 0 up to 255.
fn luma(r: u8, g: u8, b: u8) -> f32 {
    0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)
}

/// Each section's colour: NEEDS YOU as the sidebar's "asked you" mark, CLAUDE as an agent at
/// work, WAITING ON PEOPLE lilac, DONE as done. A group a newer reader added waits
/// with the people.
pub(super) fn group_role(group: TaskGroup, palette: &Palette) -> Color {
    match group {
        TaskGroup::You => palette.yellow,
        TaskGroup::Claude => palette.teal,
        TaskGroup::Waiting | TaskGroup::Unknown => lilac(palette),
        TaskGroup::Done => palette.green,
    }
}

/// Each kind's colour: features lilac, bugs red, chores teal; a kind this client does not
/// know the text colour.
pub(in crate::client::shell) fn kind_role(kind: WorkKind, palette: &Palette) -> Color {
    match kind {
        WorkKind::Feature => lilac(palette),
        WorkKind::Bug => palette.red,
        WorkKind::Chore => palette.teal,
        WorkKind::Unknown => palette.text,
    }
}

/// Each stage's colour: PLAN lilac, BUILD yellow as working is, REVIEW rose, VERIFY blue,
/// SHIP green as done is; none for a stage this client does not know.
pub(in crate::client::shell) fn stage_role(stage: WorkStage, palette: &Palette) -> Option<Color> {
    match stage {
        WorkStage::Plan => Some(lilac(palette)),
        WorkStage::Build => Some(palette.yellow),
        WorkStage::Review => Some(rose(palette)),
        WorkStage::Check => Some(palette.blue),
        WorkStage::Shipped => Some(palette.green),
        WorkStage::Unknown => None,
    }
}

/// Each section's colour: IN PROGRESS yellow as working is, the rest as the groups they hold.
pub(super) fn section_role(section: TaskSection, palette: &Palette) -> Color {
    match section {
        TaskSection::InProgress => palette.yellow,
        TaskSection::ActionRequired => group_role(TaskGroup::You, palette),
        TaskSection::Queued => group_role(TaskGroup::Claude, palette),
        TaskSection::Waiting | TaskSection::Unknown => group_role(TaskGroup::Waiting, palette),
        TaskSection::Completed => group_role(TaskGroup::Done, palette),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fill_mixes_its_role_into_the_ground_and_falls_back_without_rgb() {
        let mut palette = Palette::catppuccin();
        palette.panel_bg = Color::Rgb(0, 0, 0);
        assert_eq!(
            tint(Color::Rgb(200, 100, 50), 0.5, &palette, Color::Reset),
            Color::Rgb(100, 50, 25)
        );
        assert_eq!(
            tint(Color::Indexed(3), 0.5, &palette, Color::Red),
            Color::Red
        );
        // A theme that leaves the panel to the terminal mixes into its darkest surface.
        palette.panel_bg = Color::Reset;
        palette.surface_dim = Color::Rgb(10, 10, 10);
        assert_eq!(
            tint(Color::Rgb(110, 110, 110), 0.5, &palette, Color::Reset),
            Color::Rgb(60, 60, 60)
        );
    }

    /// The role drawn lilac: WAITING ON PEOPLE.
    fn lilac_role(palette: &Palette) -> Color {
        group_role(TaskGroup::Waiting, palette)
    }

    #[test]
    fn the_waiting_roles_draw_a_lilac_where_the_themes_mauve_is_not_one() {
        // Vesper's mauve is a light peach beside its yellow, so WAITING read as NEEDS YOU.
        let vesper = Palette::vesper();
        assert_eq!(lilac_role(&vesper), Color::Rgb(196, 167, 231));
        // A theme whose mauve is a lilac, pink or magenta keeps it, and so does a theme left
        // to the terminal's own colours.
        for name in crate::config::THEME_NAMES {
            let palette = Palette::from_name(name).expect("a built-in theme");
            if *name != "vesper" {
                assert_eq!(lilac_role(&palette), palette.mauve, "{name}");
            }
        }
        // A violet too close to the yellow is no help either; on a light ground the lilac is
        // a deep one.
        let mut light = Palette::catppuccin_latte();
        light.yellow = Color::Rgb(140, 70, 230);
        assert_eq!(lilac_role(&light), Color::Rgb(124, 77, 196));
    }

    #[test]
    fn review_is_a_rose_at_the_lilacs_lightness() {
        assert_eq!(rose(&Palette::vesper()), Color::Rgb(255, 146, 160));
        assert_eq!(rose(&Palette::catppuccin()), Color::Rgb(255, 149, 163));
        // A lilac the terminal colours: a rose of its own kind.
        assert_eq!(rose(&Palette::terminal()), Color::Magenta);
    }

    #[test]
    fn review_reads_apart_from_every_other_stage_and_a_bug_is_red_on_every_theme() {
        let stages = [
            WorkStage::Plan,
            WorkStage::Build,
            WorkStage::Check,
            WorkStage::Shipped,
        ];
        for name in crate::config::THEME_NAMES {
            let palette = Palette::from_name(name).expect("a built-in theme");
            let review = stage_role(WorkStage::Review, &palette);
            for stage in stages {
                assert_ne!(review, stage_role(stage, &palette), "{name}: {stage:?}");
            }
            assert_eq!(kind_role(WorkKind::Bug, &palette), palette.red, "{name}");
        }
    }

    #[test]
    fn the_five_stages_are_apart_on_catppuccin_vesper_and_the_terminal() {
        let stages = [
            WorkStage::Plan,
            WorkStage::Build,
            WorkStage::Review,
            WorkStage::Check,
            WorkStage::Shipped,
        ];
        let mut vesper = Palette::vesper();
        // The author's own blue.
        vesper.blue = Color::Rgb(0x7f, 0xb4, 0xca);
        for palette in [
            Palette::catppuccin(),
            Palette::vesper(),
            vesper,
            Palette::terminal(),
        ] {
            let colours: Vec<Option<Color>> = stages
                .iter()
                .map(|stage| stage_role(*stage, &palette))
                .collect();
            for (at, colour) in colours.iter().enumerate() {
                for (other, other_colour) in colours.iter().enumerate().skip(at + 1) {
                    assert_ne!(
                        colour, other_colour,
                        "{:?} and {:?}",
                        stages[at], stages[other]
                    );
                }
            }
        }
    }
}
