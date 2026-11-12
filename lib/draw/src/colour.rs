// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Colours, and mixing one colour over another.

/// A colour: red, green and blue from 0 to 255 each, and **alpha**, how solid it is, from 0
/// (clear: drawing it changes nothing) to 255 (solid: it replaces what was there). In between,
/// what was there shows through.
///
/// It is one 32-bit number, `0xAARRGGBB`: alpha in the top byte, then red, green and blue. The
/// alpha is *not* multiplied into the other three ("straight" alpha), so half-clear red is
/// `0x80FF0000`.
///
/// A struct with one unnamed field like this is a Rust *newtype*: it costs nothing more than
/// the number inside, but the compiler won't let a colour be mistaken for any other `u32`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Colour(pub u32);

impl Colour {
    /// Completely clear.
    pub const CLEAR: Colour = Colour(0);
    /// Solid black.
    pub const BLACK: Colour = Colour(0xFF00_0000);
    /// Solid white.
    pub const WHITE: Colour = Colour(0xFFFF_FFFF);

    /// A solid colour from `0xRRGGBB`, the way colours are usually written (and the way 1.x's
    /// code writes them). Anything in the top byte is ignored.
    pub const fn solid(rgb: u32) -> Colour {
        Colour(0xFF00_0000 | rgb)
    }

    /// A solid colour from its red, green and blue.
    pub const fn rgb(red: u8, green: u8, blue: u8) -> Colour {
        Colour::argb(255, red, green, blue)
    }

    /// A colour from its alpha, red, green and blue.
    pub const fn argb(alpha: u8, red: u8, green: u8, blue: u8) -> Colour {
        Colour((alpha as u32) << 24 | (red as u32) << 16 | (green as u32) << 8 | blue as u32)
    }

    /// The same colour with another alpha.
    pub const fn with_alpha(self, alpha: u8) -> Colour {
        Colour(self.0 & 0x00FF_FFFF | (alpha as u32) << 24)
    }

    /// The same colour, `alpha`/255 as solid as it was: for fading something that may already
    /// be partly clear.
    pub const fn faded(self, alpha: u8) -> Colour {
        self.with_alpha(div255(self.alpha() as u32 * alpha as u32) as u8)
    }

    /// How solid it is, 0 to 255.
    pub const fn alpha(self) -> u8 {
        (self.0 >> 24) as u8
    }

    /// Its red.
    pub const fn red(self) -> u8 {
        (self.0 >> 16) as u8
    }

    /// Its green.
    pub const fn green(self) -> u8 {
        (self.0 >> 8) as u8
    }

    /// Its blue.
    pub const fn blue(self) -> u8 {
        self.0 as u8
    }

    /// The pixel that results from drawing this colour over the pixel `under`.
    ///
    /// Each of red, green and blue becomes `(colour × alpha + under × (255 − alpha)) / 255`,
    /// rounded to the nearest whole number, so alpha 255 gives exactly this colour and alpha 0
    /// leaves exactly what was there. The result's top byte is 255: a canvas is always solid
    /// (see [`crate::Canvas`]), whatever `under`'s top byte was.
    #[inline]
    pub const fn over(self, under: u32) -> u32 {
        let alpha = self.0 >> 24;
        if alpha == 255 {
            return self.0;
        }
        let rest = 255 - alpha;
        // Red and blue are worked out together, each in its own 16 bits of one number: a
        // product of two bytes fits in 16 bits, so the two never spill into each other. That
        // halves the multiplications, which matters when every pixel of a window is blended.
        let red_blue = (self.0 & 0x00FF_00FF) * alpha + (under & 0x00FF_00FF) * rest;
        let green = ((self.0 >> 8) & 0xFF) * alpha + ((under >> 8) & 0xFF) * rest;
        // `div255`, on both halves at once.
        let t = red_blue + 0x0080_0080;
        let red_blue = ((t + ((t >> 8) & 0x00FF_00FF)) >> 8) & 0x00FF_00FF;
        0xFF00_0000 | red_blue | div255(green) << 8
    }

    /// A colour between `from` and `to`: `step` steps of `steps` along the way, each of alpha,
    /// red, green and blue rounded to the nearest whole number. Step 0 (or less) is exactly
    /// `from`; step `steps` (or more) is exactly `to`. With no steps at all it is `from`.
    pub fn between(from: Colour, to: Colour, step: i32, steps: i32) -> Colour {
        if steps <= 0 || step <= 0 {
            return from;
        }
        if step >= steps {
            return to;
        }
        // 64-bit so that a byte times any `i32` can't overflow.
        let (step, steps) = (i64::from(step), i64::from(steps));
        let channel = |shift: u32| -> u32 {
            let (a, b) = (i64::from(from.0 >> shift & 0xFF), i64::from(to.0 >> shift & 0xFF));
            ((a * (steps - step) + b * step + steps / 2) / steps) as u32
        };
        Colour(channel(24) << 24 | channel(16) << 16 | channel(8) << 8 | channel(0))
    }
}

/// `x / 255` rounded to the nearest whole number, for `x` up to 255 × 255, without a division
/// (which is far slower than adding and shifting). A `const fn` can also be run by the
/// compiler, for constants.
#[inline]
pub(crate) const fn div255(x: u32) -> u32 {
    let t = x + 128;
    (t + (t >> 8)) >> 8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn div255_rounds_to_nearest_for_every_product_of_two_bytes() {
        for x in 0..=255 * 255u32 {
            assert_eq!(div255(x), (2 * x + 255) / 510, "{x}");
        }
    }

    #[test]
    fn making_and_taking_apart() {
        let c = Colour::argb(0x12, 0x34, 0x56, 0x78);
        assert_eq!(c.0, 0x1234_5678);
        assert_eq!((c.alpha(), c.red(), c.green(), c.blue()), (0x12, 0x34, 0x56, 0x78));
        assert_eq!(Colour::rgb(1, 2, 3).0, 0xFF01_0203);
        assert_eq!(Colour::solid(0x2f6b55).0, 0xFF2F_6B55);
        assert_eq!(Colour::solid(0x122f_6b55).0, 0xFF2F_6B55, "the top byte is ignored");
        assert_eq!(c.with_alpha(0xFF).0, 0xFF34_5678);
        assert_eq!(Colour::solid(0xABCDEF).faded(128).0, 0x80AB_CDEF);
        assert_eq!(Colour(0x80AB_CDEF).faded(128).0, 0x40AB_CDEF);
        assert_eq!(Colour::WHITE.faded(0), Colour(0x00FF_FFFF));
        assert_eq!(Colour::WHITE.faded(255), Colour::WHITE);
    }

    /// The slow, obvious way, one channel at a time.
    fn over_model(colour: u32, under: u32) -> u32 {
        let alpha = colour >> 24;
        let channel = |shift: u32| {
            let (c, u) = (colour >> shift & 0xFF, under >> shift & 0xFF);
            (2 * (c * alpha + u * (255 - alpha)) + 255) / 510
        };
        0xFF00_0000 | channel(16) << 16 | channel(8) << 8 | channel(0)
    }

    #[test]
    fn over_matches_the_model_for_every_alpha_and_every_pair_of_bytes() {
        // The three channels are worked out independently, so trying every pair of bytes in
        // all three at once covers each of them.
        for alpha in 0..=255u32 {
            for c in 0..=255u32 {
                for u in (0..=255u32).step_by(3).chain([255]) {
                    let colour = alpha << 24 | c << 16 | (255 - c) << 8 | c;
                    let under = 0x5A00_0000 | u << 16 | u << 8 | (255 - u);
                    assert_eq!(
                        Colour(colour).over(under),
                        over_model(colour, under),
                        "{colour:#010x} over {under:#010x}"
                    );
                }
            }
        }
    }

    #[test]
    fn solid_replaces_and_clear_leaves_alone() {
        assert_eq!(Colour(0xFF12_3456).over(0x00AB_CDEF), 0xFF12_3456);
        assert_eq!(Colour(0x0012_3456).over(0x00AB_CDEF), 0xFFAB_CDEF);
        assert_eq!(Colour(0x0012_3456).over(0x77AB_CDEF), 0xFFAB_CDEF);
        // Half black over white, and half white over black, land in the middle.
        assert_eq!(Colour(0x8000_0000).over(0xFFFF_FFFF), 0xFF7F_7F7F);
        assert_eq!(Colour(0x80FF_FFFF).over(0xFF00_0000), 0xFF80_8080);
        // A colour over itself is itself, at any alpha.
        for alpha in 0..=255u32 {
            assert_eq!(Colour(alpha << 24 | 0x0037_C8FF).over(0xFF37_C8FF), 0xFF37_C8FF);
        }
    }

    #[test]
    fn between_runs_from_one_colour_to_the_other() {
        let (a, b) = (Colour(0xFF2F_6B55), Colour(0x0022_503E));
        assert_eq!(Colour::between(a, b, 0, 25), a);
        assert_eq!(Colour::between(a, b, 25, 25), b);
        assert_eq!(Colour::between(a, b, -3, 25), a);
        assert_eq!(Colour::between(a, b, 26, 25), b);
        assert_eq!(Colour::between(a, b, 5, 0), a);
        assert_eq!(Colour::between(a, b, 5, -1), a);
        assert_eq!(Colour::between(Colour::BLACK, Colour::WHITE, 1, 2), Colour(0xFF80_8080));
        assert_eq!(Colour::between(Colour::WHITE, Colour::BLACK, 1, 2), Colour(0xFF80_8080));
        assert_eq!(Colour::between(Colour::BLACK, Colour::WHITE, 1, 3), Colour(0xFF55_5555));
        // Every step is between its neighbours, channel by channel, going down here.
        let mut last = a;
        for step in 1..=25 {
            let c = Colour::between(a, b, step, 25);
            assert!(c.alpha() <= last.alpha() && c.red() <= last.red());
            assert!(c.green() <= last.green() && c.blue() <= last.blue());
            last = c;
        }
        // Huge step counts don't overflow.
        let c = Colour::between(Colour::BLACK, Colour::WHITE, i32::MAX / 2, i32::MAX);
        assert!(c == Colour(0xFF7F_7F7F) || c == Colour(0xFF80_8080));
    }
}
