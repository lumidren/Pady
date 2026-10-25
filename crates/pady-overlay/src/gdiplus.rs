//! Owned wrappers over the handful of GDI+ flat-API objects the overlay
//! draws with. Each is deleted on drop, so frames can be drawn thousands of
//! times without leaking, and every failure names the call that failed.
//!
//! All of them belong to the thread that made them (the raw pointers keep
//! them `!Send`) and must be dropped before that thread's [`Session`].

use crate::color::Color;
use crate::layout::Rect;
use std::fmt;
use std::ptr::null_mut;
use windows::core::PCWSTR;
use windows::Win32::Graphics::GdiPlus as gp;

/// `PixelFormat32bppPARGB`, which the `windows` crate doesn't export:
/// premultiplied BGRA, exactly what `UpdateLayeredWindow` reads.
const PIXEL_FORMAT_32BPP_PARGB: i32 = 0x000E_200B;

pub const REGULAR: i32 = gp::FontStyleRegular.0;
pub const BOLD: i32 = gp::FontStyleBold.0;

/// A GDI+ call that failed, with its `Status` (-1: it returned no object).
#[derive(Debug)]
pub struct Error {
    call: &'static str,
    status: i32,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed (GDI+ status {})", self.call, self.status)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn check(call: &'static str, status: gp::Status) -> Result<()> {
    if status.0 == 0 {
        Ok(())
    } else {
        Err(Error { call, status: status.0 })
    }
}

/// The object a constructor returned through its out-pointer, if it succeeded.
fn created<T>(call: &'static str, status: gp::Status, object: *mut T) -> Result<*mut T> {
    check(call, status)?;
    if object.is_null() {
        Err(Error { call, status: -1 })
    } else {
        Ok(object)
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// GDI+ started on this thread; shut down when dropped.
pub struct Session(usize);

impl Session {
    pub fn start() -> Result<Session> {
        let input = gp::GdiplusStartupInput {
            GdiplusVersion: 1,
            DebugEventCallback: 0,
            SuppressBackgroundThread: false.into(),
            SuppressExternalCodecs: true.into(),
        };
        let mut token = 0;
        check("GdiplusStartup", unsafe { gp::GdiplusStartup(&mut token, &input, null_mut()) })?;
        Ok(Session(token))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe { gp::GdiplusShutdown(self.0) }
    }
}

macro_rules! owned {
    ($(#[$meta:meta])* $name:ident($raw:ty) => $delete:path) => {
        $(#[$meta])*
        pub struct $name(*mut $raw);

        impl Drop for $name {
            fn drop(&mut self) {
                let _ = unsafe { $delete(self.0.cast()) };
            }
        }
    };
}

owned!(
    /// A bitmap over pixels owned by someone else (see [`Bitmap::over_pixels`]).
    Bitmap(gp::GpBitmap) => gp::GdipDisposeImage
);
owned!(Graphics(gp::GpGraphics) => gp::GdipDeleteGraphics);
owned!(Path(gp::GpPath) => gp::GdipDeletePath);
owned!(
    /// A solid or gradient brush.
    Brush(gp::GpBrush) => gp::GdipDeleteBrush
);
owned!(Pen(gp::GpPen) => gp::GdipDeletePen);
owned!(FontFamily(gp::GpFontFamily) => gp::GdipDeleteFontFamily);
owned!(Font(gp::GpFont) => gp::GdipDeleteFont);
owned!(StringFormat(gp::GpStringFormat) => gp::GdipDeleteStringFormat);

impl Bitmap {
    /// GDI+'s view of `width`×`height` premultiplied BGRA pixels, top row
    /// first, so it draws straight into memory a layered window can use.
    ///
    /// # Safety
    /// `bits` must point to `width × height × 4` writable bytes that outlive
    /// the bitmap.
    pub unsafe fn over_pixels(width: i32, height: i32, bits: *mut u8) -> Result<Bitmap> {
        let mut bitmap = null_mut();
        let status =
            gp::GdipCreateBitmapFromScan0(width, height, width * 4, PIXEL_FORMAT_32BPP_PARGB, Some(bits), &mut bitmap);
        created("GdipCreateBitmapFromScan0", status, bitmap).map(Bitmap)
    }

    /// Writes the bitmap as a PNG (for eyeballing renders in tests).
    #[cfg(test)]
    pub fn save_png(&self, path: &std::path::Path) -> Result<()> {
        use std::os::windows::ffi::OsStrExt;
        // GDI+'s built-in PNG encoder.
        const PNG: windows::core::GUID = windows::core::GUID::from_u128(0x557cf406_1a04_11d3_9a73_0000f81ef32e);
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let status = unsafe { gp::GdipSaveImageToFile(self.0.cast(), PCWSTR(path.as_ptr()), &PNG, std::ptr::null()) };
        check("GdipSaveImageToFile", status)
    }
}

impl Graphics {
    /// Draws into `bitmap`: antialiased shapes on true pixel centres, plain
    /// (browser-like, not gamma-corrected) blending, and grayscale text,
    /// since ClearType needs an opaque background.
    pub fn for_bitmap(bitmap: &Bitmap) -> Result<Graphics> {
        let mut graphics = null_mut();
        let status = unsafe { gp::GdipGetImageGraphicsContext(bitmap.0.cast(), &mut graphics) };
        let g = Graphics(created("GdipGetImageGraphicsContext", status, graphics)?);
        unsafe {
            // 8×8 antialiasing needs GDI+ 1.1; GDI+ 1.0 still has the 4×4 mode.
            if gp::GdipSetSmoothingMode(g.0, gp::SmoothingModeAntiAlias8x8).0 != 0 {
                check("GdipSetSmoothingMode", gp::GdipSetSmoothingMode(g.0, gp::SmoothingModeAntiAlias))?;
            }
            check("GdipSetPixelOffsetMode", gp::GdipSetPixelOffsetMode(g.0, gp::PixelOffsetModeHighQuality))?;
            check("GdipSetCompositingQuality", gp::GdipSetCompositingQuality(g.0, gp::CompositingQualityAssumeLinear))?;
            check(
                "GdipSetTextRenderingHint",
                gp::GdipSetTextRenderingHint(g.0, gp::TextRenderingHintAntiAliasGridFit),
            )?;
        }
        Ok(g)
    }

    pub fn fill_path(&self, brush: &Brush, path: &Path) -> Result<()> {
        check("GdipFillPath", unsafe { gp::GdipFillPath(self.0, brush.0, path.0) })
    }

    pub fn fill_rect(&self, brush: &Brush, r: Rect) -> Result<()> {
        check("GdipFillRectangle", unsafe { gp::GdipFillRectangle(self.0, brush.0, r.x, r.y, r.w, r.h) })
    }

    pub fn draw_path(&self, pen: &Pen, path: &Path) -> Result<()> {
        check("GdipDrawPath", unsafe { gp::GdipDrawPath(self.0, pen.0, path.0) })
    }

    pub fn clip_to(&self, path: &Path) -> Result<()> {
        check("GdipSetClipPath", unsafe { gp::GdipSetClipPath(self.0, path.0, gp::CombineModeReplace) })
    }

    pub fn reset_clip(&self) -> Result<()> {
        check("GdipResetClip", unsafe { gp::GdipResetClip(self.0) })
    }

    /// Advance width of `text` (UTF-16) in `font`.
    pub fn measure(&self, text: &[u16], font: &Font, format: &StringFormat) -> Result<f32> {
        let layout = gp::RectF { X: 0.0, Y: 0.0, Width: 10_000.0, Height: 10_000.0 };
        let mut bounds = gp::RectF::default();
        let status = unsafe {
            gp::GdipMeasureString(
                self.0,
                PCWSTR(text.as_ptr()),
                text.len() as i32,
                font.0,
                &layout,
                format.0,
                &mut bounds,
                null_mut(),
                null_mut(),
            )
        };
        check("GdipMeasureString", status).map(|()| bounds.Width)
    }

    /// Draws `text` (UTF-16) with its line box's top-left at `at` (its size
    /// only matters for centred formats).
    pub fn draw_text(&self, text: &[u16], font: &Font, at: Rect, format: &StringFormat, brush: &Brush) -> Result<()> {
        let rect = gp::RectF { X: at.x, Y: at.y, Width: at.w, Height: at.h };
        let status = unsafe {
            gp::GdipDrawString(self.0, PCWSTR(text.as_ptr()), text.len() as i32, font.0, &rect, format.0, brush.0)
        };
        check("GdipDrawString", status)
    }

    /// Finishes queued drawing, before the pixels are read or shown.
    pub fn flush(&self) -> Result<()> {
        check("GdipFlush", unsafe { gp::GdipFlush(self.0, gp::FlushIntentionSync) })
    }
}

impl Path {
    pub fn new() -> Result<Path> {
        let mut path = null_mut();
        let status = unsafe { gp::GdipCreatePath(gp::FillModeAlternate, &mut path) };
        created("GdipCreatePath", status, path).map(Path)
    }

    /// Makes this path a rectangle with corners of `radius` (shrunk to fit,
    /// so half the height gives a pill).
    pub fn set_rounded_rect(&mut self, r: Rect, radius: f32) -> Result<()> {
        let radius = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
        unsafe {
            check("GdipResetPath", gp::GdipResetPath(self.0))?;
            if radius <= 0.0 {
                return check("GdipAddPathRectangle", gp::GdipAddPathRectangle(self.0, r.x, r.y, r.w, r.h));
            }
            let d = radius * 2.0;
            let (right, bottom) = (r.right() - d, r.y + r.h - d);
            for (x, y, start) in [(r.x, r.y, 180.0), (right, r.y, 270.0), (right, bottom, 0.0), (r.x, bottom, 90.0)] {
                check("GdipAddPathArc", gp::GdipAddPathArc(self.0, x, y, d, d, start, 90.0))?;
            }
            check("GdipClosePathFigure", gp::GdipClosePathFigure(self.0))
        }
    }
}

impl Brush {
    pub fn solid(color: Color) -> Result<Brush> {
        let mut brush = null_mut();
        let status = unsafe { gp::GdipCreateSolidFill(color.argb(), &mut brush) };
        created("GdipCreateSolidFill", status, brush).map(|b| Brush(b.cast()))
    }

    /// A left-to-right gradient from `x0` to `x1` through `stops` (offset
    /// 0–1, colour), like CSS `linear-gradient(90deg, …)`.
    pub fn horizontal(x0: f32, x1: f32, stops: &[(f32, Color)]) -> Result<Brush> {
        Brush::linear(gp::PointF { X: x0, Y: 0.0 }, gp::PointF { X: x1.max(x0 + 0.01), Y: 0.0 }, stops)
    }

    /// A top-to-bottom gradient across `rect`, like `linear-gradient(180deg, …)`.
    pub fn vertical(rect: Rect, stops: &[(f32, Color)]) -> Result<Brush> {
        let bottom = (rect.y + rect.h).max(rect.y + 0.01);
        Brush::linear(gp::PointF { X: 0.0, Y: rect.y }, gp::PointF { X: 0.0, Y: bottom }, stops)
    }

    fn linear(from: gp::PointF, to: gp::PointF, stops: &[(f32, Color)]) -> Result<Brush> {
        let (Some(first), Some(last)) = (stops.first(), stops.last()) else {
            return Err(Error { call: "GdipCreateLineBrush", status: -1 });
        };
        let mut brush = null_mut();
        // Mirroring rather than repeating past the ends keeps the edge pixels
        // from picking up the far end's colour.
        let status = unsafe {
            gp::GdipCreateLineBrush(&from, &to, first.1.argb(), last.1.argb(), gp::WrapModeTileFlipXY, &mut brush)
        };
        let gradient = created("GdipCreateLineBrush", status, brush)?;
        let brush = Brush(gradient.cast());
        if stops.len() > 2 {
            let colors: Vec<u32> = stops.iter().map(|s| s.1.argb()).collect();
            let offsets: Vec<f32> = stops.iter().map(|s| s.0).collect();
            let status =
                unsafe { gp::GdipSetLinePresetBlend(gradient, colors.as_ptr(), offsets.as_ptr(), stops.len() as i32) };
            check("GdipSetLinePresetBlend", status)?;
        }
        Ok(brush)
    }
}

impl Pen {
    pub fn new(color: Color, width: f32) -> Result<Pen> {
        let mut pen = null_mut();
        let status = unsafe { gp::GdipCreatePen1(color.argb(), width, gp::UnitPixel, &mut pen) };
        created("GdipCreatePen1", status, pen).map(Pen)
    }
}

impl FontFamily {
    /// An installed font family, if it is installed.
    pub fn named(name: &str) -> Result<FontFamily> {
        let name = wide(name);
        let mut family = null_mut();
        let status = unsafe { gp::GdipCreateFontFamilyFromName(PCWSTR(name.as_ptr()), null_mut(), &mut family) };
        created("GdipCreateFontFamilyFromName", status, family).map(FontFamily)
    }

    /// Cell ascent and descent in ems: where the baseline sits in a line.
    pub fn metrics(&self, style: i32) -> Result<(f32, f32)> {
        let (mut em, mut ascent, mut descent) = (0u16, 0u16, 0u16);
        unsafe {
            check("GdipGetEmHeight", gp::GdipGetEmHeight(self.0, style, &mut em))?;
            check("GdipGetCellAscent", gp::GdipGetCellAscent(self.0, style, &mut ascent))?;
            check("GdipGetCellDescent", gp::GdipGetCellDescent(self.0, style, &mut descent))?;
        }
        let em = em.max(1) as f32;
        Ok((ascent as f32 / em, descent as f32 / em))
    }
}

impl Font {
    /// `family` at `px` pixels per em.
    pub fn new(family: &FontFamily, px: f32, style: i32) -> Result<Font> {
        let mut font = null_mut();
        let status = unsafe { gp::GdipCreateFont(family.0, px, style, gp::UnitPixel, &mut font) };
        created("GdipCreateFont", status, font).map(Font)
    }
}

impl StringFormat {
    /// Typographic layout (none of GDI+'s default padding around text), one
    /// line, never clipped, aligned `align` horizontally. Unlike the generic
    /// format it has no `LineLimit`, which draws nothing at all when the
    /// layout box is a hair shorter than GDI+'s idea of the line height.
    pub fn typographic(align: gp::StringAlignment) -> Result<StringFormat> {
        let mut generic = null_mut();
        check("GdipStringFormatGetGenericTypographic", unsafe {
            gp::GdipStringFormatGetGenericTypographic(&mut generic)
        })?;
        // The generic format is shared by the whole process: change a copy.
        let mut copy = null_mut();
        let status = unsafe { gp::GdipCloneStringFormat(generic, &mut copy) };
        let format = StringFormat(created("GdipCloneStringFormat", status, copy)?);
        let flags = gp::StringFormatFlagsNoWrap.0 | gp::StringFormatFlagsNoClip.0;
        unsafe {
            check("GdipSetStringFormatFlags", gp::GdipSetStringFormatFlags(format.0, flags))?;
            check("GdipSetStringFormatAlign", gp::GdipSetStringFormatAlign(format.0, align))?;
        }
        Ok(format)
    }
}
