//! ImageIO decoding (HEIC / HEIF, AVIF, and whatever else the system reads). Thread-safe: each
//! call uses its own image source and bitmap context.

use std::ffi::c_void;

use objc2_core_foundation::{CFBoolean, CFData, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGColorSpaceModel, CGContext, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_image_io::{
    CGImageSource, kCGImagePropertyDepth, kCGImagePropertyOrientation, kCGImagePropertyPixelHeight, kCGImagePropertyPixelWidth,
    kCGImageSourceCreateThumbnailFromImageAlways, kCGImageSourceCreateThumbnailWithTransform, kCGImageSourceThumbnailMaxPixelSize,
};

use crate::{Image, Pixels};

/// Refuse images with more pixels than this (as `lightcraft-codecs` does).
const MAX_PIXELS: usize = 1 << 30;

/// An integer property of the image (`PixelWidth`, `Orientation`…).
fn int_property(props: &CFDictionary, key: &CFString) -> Option<i64> {
    let key: *const CFString = key;
    // SAFETY: `props` is an ImageIO properties dictionary (CFString keys, CFType values); a
    // missing key gives null. The value is borrowed from the dictionary, which outlives it here.
    let value = unsafe { props.value(key.cast::<c_void>()) };
    // SAFETY: a non-null value of a CF dictionary is a valid CF object.
    let value: &CFType = unsafe { value.cast::<CFType>().as_ref() }?;
    value.downcast_ref::<CFNumber>()?.as_i64()
}

pub fn decode(bytes: &[u8], max: Option<(u32, u32)>) -> Result<Image, String> {
    let data = CFData::from_bytes(bytes);
    // SAFETY: a CFData of the file's bytes; no options.
    let source = unsafe { CGImageSource::with_data(&data, None) }.ok_or("ImageIO can't read this file")?;
    // SAFETY: a valid image source.
    if unsafe { source.count() } == 0 {
        return Err("ImageIO found no image in this file".into());
    }
    // SAFETY: index 0 exists (count > 0); no options.
    let props = unsafe { source.properties_at_index(0, None) }.ok_or("ImageIO can't read the image's properties")?;
    let dim = |k: &CFString| int_property(&props, k).and_then(|v| u32::try_from(v).ok()).filter(|v| *v > 0);
    // SAFETY: reading ImageIO's property-key constants (immutable framework statics).
    let (kw, kh, ko, kd) = unsafe { (kCGImagePropertyPixelWidth, kCGImagePropertyPixelHeight, kCGImagePropertyOrientation, kCGImagePropertyDepth) };
    let (sw, sh) = (dim(kw).ok_or("the image has no width")?, dim(kh).ok_or("the image has no height")?);
    let orientation = int_property(&props, ko).and_then(|v| u16::try_from(v).ok()).filter(|v| (1..=8).contains(v)).unwrap_or(1);
    let depth = int_property(&props, kd).and_then(|v| u8::try_from(v).ok()).unwrap_or(8);

    let image: CFRetained<CGImage> = match max {
        // a thumbnail: ImageIO decodes straight to about that size (HEIC tiles, embedded previews)
        Some((mw, mh)) if mw.max(mh) > 0 && mw.max(mh) < sw.max(sh) => {
            let size = CFNumber::new_i64(i64::from(mw.max(mh)));
            // SAFETY: reading ImageIO's option-key constants (immutable framework statics).
            let keys = unsafe {
                [kCGImageSourceCreateThumbnailFromImageAlways, kCGImageSourceThumbnailMaxPixelSize, kCGImageSourceCreateThumbnailWithTransform]
            };
            let values: [&CFType; 3] = [CFBoolean::new(true).as_ref(), size.as_ref(), CFBoolean::new(false).as_ref()];
            let opts = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
            // SAFETY: index 0 exists; the options are ImageIO's thumbnail keys with the value
            // types it documents (booleans and a number); orientation is left to the caller.
            unsafe { source.thumbnail_at_index(0, Some(opts.as_ref())) }
        }
        // SAFETY: index 0 exists; no options.
        _ => unsafe { source.image_at_index(0, None) },
    }
    .ok_or("ImageIO couldn't decode the image")?;
    let (w, h) = (CGImage::width(Some(&image)), CGImage::height(Some(&image)));
    let n = w.checked_mul(h).filter(|n| *n > 0 && *n <= MAX_PIXELS).ok_or(format!("unsupported image size {w}×{h}"))?;
    let (wu, hu) = (u32::try_from(w).map_err(|e| e.to_string())?, u32::try_from(h).map_err(|e| e.to_string())?);

    // draw into a bitmap in the image's own RGB space (Display P3 for iPhone photos), so no colour
    // is lost to sRGB; anything else (grey, CMYK) is converted to sRGB
    let own = CGImage::color_space(Some(&image)).filter(|s| CGColorSpace::model(Some(s)) == CGColorSpaceModel::RGB);
    // SAFETY: reading CoreGraphics' colour-space name constant (an immutable framework static).
    let space = own.clone().or_else(|| CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))).ok_or("no colour space to decode into")?;
    let icc = own.as_deref().and_then(|s| CGColorSpace::icc_data(Some(s))).map(|d| d.to_vec());
    let rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64));
    let premultiplied_last = CGImageAlphaInfo::PremultipliedLast.0;

    if depth > 8 {
        // 10- and 12-bit HEIC: 16 bits per sample
        let mut px = vec![0u16; n * 4];
        // SAFETY: `px` holds exactly `h` rows of `w * 8` bytes and outlives the context (dropped
        // before `px` is moved); 16-bit RGBA, premultiplied alpha last, little-endian samples is a
        // pixel format CoreGraphics supports.
        let ctx: Option<CFRetained<CGContext>> = unsafe {
            CGBitmapContextCreate(px.as_mut_ptr().cast(), w, h, 16, w * 8, Some(&space), premultiplied_last | CGImageByteOrderInfo::Order16Little.0)
        };
        if let Some(ctx) = ctx {
            CGContext::draw_image(Some(&ctx), rect, Some(&image));
            drop(ctx);
            return Ok(Image {
                width: wu,
                height: hu,
                pixels: Pixels::Rgba16(px),
                bit_depth: depth.min(16),
                icc,
                orientation,
                source_width: sw,
                source_height: sh,
            });
        }
    }
    let mut px = vec![0u8; n * 4];
    // SAFETY: `px` holds exactly `h` rows of `w * 4` bytes and outlives the context (dropped
    // before `px` is moved); 8-bit RGBA with premultiplied alpha last is CoreGraphics' basic format.
    let ctx: Option<CFRetained<CGContext>> =
        unsafe { CGBitmapContextCreate(px.as_mut_ptr().cast(), w, h, 8, w * 4, Some(&space), premultiplied_last) };
    let ctx = ctx.ok_or("CoreGraphics couldn't make a bitmap for the image")?;
    CGContext::draw_image(Some(&ctx), rect, Some(&image));
    drop(ctx);
    Ok(Image { width: wu, height: hu, pixels: Pixels::Rgba8(px), bit_depth: depth.min(8), icc, orientation, source_width: sw, source_height: sh })
}
