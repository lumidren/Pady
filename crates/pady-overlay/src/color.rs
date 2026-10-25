//! Colours, and the two palettes copied from `pady-app/ui/overlay.css`.

use crate::shadow::Shadow;

/// A colour like CSS `rgba()`: sRGB channels 0–255 and a straight alpha 0–1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
        Color { r: r as f32, g: g as f32, b: b as f32, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color::rgba(r, g, b, 1.0)
    }

    /// The same colour with its alpha multiplied by `k` (0–1).
    pub fn fade(self, k: f32) -> Color {
        Color { a: self.a * k.clamp(0.0, 1.0), ..self }
    }

    /// `t` of the way from `self` to `to`. Mixes premultiplied, like CSS
    /// transitions and gradients, so fading towards a transparent colour
    /// never passes through a darker one.
    pub fn lerp(self, to: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        let a = self.a + (to.a - self.a) * t;
        if a <= 0.0 {
            return Color { a: 0.0, ..to };
        }
        let mix = |x: f32, y: f32| (x * self.a + (y * to.a - x * self.a) * t) / a;
        Color { r: mix(self.r, to.r), g: mix(self.g, to.g), b: mix(self.b, to.b), a }
    }

    /// `0xAARRGGBB` with straight alpha, as GDI+ brushes and pens take it.
    pub fn argb(self) -> u32 {
        pack(self.a * 255.0, self.r, self.g, self.b)
    }

    /// `0xAARRGGBB` premultiplied, as a layered window's bitmap stores it.
    pub fn premultiplied(self) -> u32 {
        let a = self.a.clamp(0.0, 1.0);
        pack(a * 255.0, self.r * a, self.g * a, self.b * a)
    }
}

fn pack(a: f32, r: f32, g: f32, b: f32) -> u32 {
    let byte = |v: f32| v.round().clamp(0.0, 255.0) as u32;
    byte(a) << 24 | byte(r) << 16 | byte(g) << 8 | byte(b)
}

/// One theme's colours (the custom properties at the top of overlay.css).
#[derive(Debug, PartialEq)]
pub struct Palette {
    /// The pill: nearly opaque, since nothing behind a layered window can be blurred.
    pub glass: Color,
    pub border: Color,
    /// Sheen over the top half of the pill.
    pub shine: Color,
    pub text: Color,
    /// The "%" sign, and everything when muted.
    pub text_dim: Color,
    pub track: Color,
    /// The bar's gradient, left to right.
    pub fill: (Color, Color),
    pub glow: Color,
    pub icon: Color,
    /// Painted first to last, the first on top (as in CSS).
    pub shadows: [Shadow; 2],
}

pub const DARK: Palette = Palette {
    glass: Color::rgba(22, 23, 31, 0.92),
    border: Color::rgba(255, 255, 255, 0.1),
    shine: Color::rgba(255, 255, 255, 0.07),
    text: Color::rgb(0xf1, 0xf1, 0xf7),
    text_dim: Color::rgba(0xf1, 0xf1, 0xf7, 0.55),
    track: Color::rgba(255, 255, 255, 0.13),
    fill: (Color::rgb(0x6d, 0x6b, 0xff), Color::rgb(0xb1, 0x4e, 0xf0)),
    glow: Color::rgba(141, 125, 255, 0.55),
    icon: Color::rgb(0xc7, 0xc0, 0xff),
    shadows: [
        Shadow { dy: 8.0, blur: 22.0, spread: -10.0, color: Color::rgba(0, 0, 0, 0.65) },
        Shadow { dy: 2.0, blur: 6.0, spread: -2.0, color: Color::rgba(0, 0, 0, 0.4) },
    ],
};

pub const LIGHT: Palette = Palette {
    glass: Color::rgba(251, 251, 253, 0.95),
    border: Color::rgba(20, 22, 40, 0.1),
    shine: Color::rgba(255, 255, 255, 0.9),
    text: Color::rgb(0x14, 0x15, 0x1d),
    text_dim: Color::rgba(20, 21, 29, 0.5),
    track: Color::rgba(20, 22, 40, 0.1),
    fill: (Color::rgb(0x5b, 0x4c, 0xf0), Color::rgb(0x9d, 0x4c, 0xe8)),
    glow: Color::rgba(91, 76, 240, 0.35),
    icon: Color::rgb(0x51, 0x41, 0xe6),
    shadows: [
        Shadow { dy: 8.0, blur: 22.0, spread: -10.0, color: Color::rgba(20, 22, 40, 0.38) },
        Shadow { dy: 2.0, blur: 6.0, spread: -2.0, color: Color::rgba(20, 22, 40, 0.16) },
    ],
};

impl Palette {
    pub fn for_theme(light: bool) -> &'static Palette {
        if light {
            &LIGHT
        } else {
            &DARK
        }
    }

    /// The bar while muted: the dim text colour, fainter (`.is-muted .pill-fill`).
    pub fn muted_fill(&self) -> Color {
        self.text_dim.fade(0.6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Color, b: Color) -> bool {
        [(a.r, b.r), (a.g, b.g), (a.b, b.b)].iter().all(|(x, y)| (x - y).abs() < 0.01) && (a.a - b.a).abs() < 1e-4
    }

    #[test]
    fn packs_straight_and_premultiplied_argb() {
        assert_eq!(Color::rgb(0x6d, 0x6b, 0xff).argb(), 0xff6d6bff);
        assert_eq!(Color::rgba(255, 0, 0, 0.5).argb(), 0x80ff0000);
        assert_eq!(Color::rgba(255, 255, 255, 0.5).premultiplied(), 0x80808080);
        assert_eq!(Color::rgba(200, 100, 50, 0.0).premultiplied(), 0);
        assert_eq!(Color::rgba(0, 0, 0, 0.65).premultiplied(), 0xa6000000);
    }

    #[test]
    fn lerp_hits_both_ends_and_the_middle() {
        let (a, b) = (Color::rgb(0, 0, 0), Color::rgba(200, 100, 50, 0.5));
        assert_eq!(a.lerp(b, 0.0), a);
        assert!(close(a.lerp(b, 1.0), b));
        // Premultiplied midpoint: alpha 0.75, colour weighted towards the more opaque end.
        assert!(close(
            a.lerp(b, 0.5),
            Color { r: 200.0 * 0.25 / 0.75, g: 100.0 * 0.25 / 0.75, b: 50.0 * 0.25 / 0.75, a: 0.75 }
        ));
    }

    #[test]
    fn lerp_towards_transparent_keeps_the_hue() {
        let red = Color::rgb(255, 0, 0);
        let half = red.lerp(Color::rgba(0, 0, 0, 0.0), 0.5);
        assert!(close(half, Color::rgba(255, 0, 0, 0.5)));
    }

    #[test]
    fn lerp_clamps_t() {
        let (a, b) = (Color::rgb(10, 20, 30), Color::rgb(40, 50, 60));
        assert_eq!(a.lerp(b, -1.0), a);
        assert!(close(a.lerp(b, 2.0), b));
    }

    #[test]
    fn fade_scales_alpha_only() {
        assert_eq!(Color::rgba(1, 2, 3, 0.5).fade(0.5), Color::rgba(1, 2, 3, 0.25));
        assert_eq!(Color::rgb(1, 2, 3).fade(7.0).a, 1.0);
    }

    #[test]
    fn themes_contrast() {
        let luma = |c: Color| 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
        for (palette, light) in [(&DARK, false), (&LIGHT, true)] {
            assert_eq!(Palette::for_theme(light), palette);
            assert_eq!(luma(palette.glass) > luma(palette.text), light, "light glass with dark text, or the reverse");
            assert!((luma(palette.glass) - luma(palette.text)).abs() > 150.0);
        }
        assert!((DARK.muted_fill().a - 0.33).abs() < 1e-6);
    }
}
