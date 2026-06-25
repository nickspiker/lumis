use std::panic;

/// Structured EXIF values for the EXIF sub-IFD (tag 0x8769), shared by the DNG/JPEG/TIFF writers.
/// All are the COMPOSITE/effective exposure (what the saved image represents). 0 / empty = omit the tag.
#[derive(Clone, Default)]
pub struct ExifData {
    pub exposure_time_s: f64,
    pub iso: f64,
    pub f_number: f64,
    pub focal_length_mm: f64,
    pub focal_length_35mm: f64,
    pub subject_distance_m: f64,
    pub datetime_original: String, // "YYYY:MM:DD HH:MM:SS" (EXIF format), empty = omit
    pub has_gps: bool,             // false = omit all GPS tags
    pub gps_lat: f64,              // signed decimal degrees
    pub gps_lon: f64,              // signed decimal degrees
    pub gps_alt: f64,              // metres (signed)
    pub image_width: u32,          // for ExifImageWidth/Height (0 = omit)
    pub image_height: u32,
}

// One EXIF IFD entry: (tag, type, count, inline_value, optional out-of-line payload). SHORT/UNDEFINED
// values live inline in the 4-byte value field; RATIONAL/ASCII go out-of-line and the value field holds
// an offset patched in by the writer.
type ExifEntry = (u16, u16, u32, u32, Option<Vec<u8>>);

// Build the EXIF tag entries from ExifData (shared by the DNG EXIF sub-IFD and the JPEG/TIFF EXIF blocks).
// Ascending tag-id order. RATIONALs use a fixed 1000 denominator scheme that preserves sub-1.0 values.
pub fn build_exif_entries(e: &ExifData) -> Vec<ExifEntry> {
    let rat = |v: f64| -> (u32, u32) {
        if v <= 0.0 {
            (0, 1)
        } else if v >= 1.0 {
            ((v * 1000.0).round() as u32, 1000)
        } else {
            (1000, (1000.0 / v).round() as u32)
        }
    };
    let ratbytes = |v: f64| -> Vec<u8> {
        let (n, d) = rat(v);
        let mut b = n.to_le_bytes().to_vec();
        b.extend(d.to_le_bytes());
        b
    };
    let mut entries: Vec<ExifEntry> = Vec::new();
    if e.exposure_time_s > 0.0 {
        entries.push((0x829A, 5, 1, 0, Some(ratbytes(e.exposure_time_s)))); // ExposureTime
    }
    if e.f_number > 0.0 {
        entries.push((0x829D, 5, 1, 0, Some(ratbytes(e.f_number)))); // FNumber
    }
    if e.iso > 0.0 {
        entries.push((0x8827, 3, 1, (e.iso.round() as u32).min(65535), None)); // ISOSpeedRatings
    }
    // STRICT ascending tag-id order from here (exiftool -validate flags any descent in an IFD).
    entries.push((0x9000, 7, 4, u32::from_le_bytes(*b"0230"), None)); // ExifVersion
    if !e.datetime_original.is_empty() {
        let mut b = e.datetime_original.clone().into_bytes();
        b.push(0);
        let cnt = b.len() as u32;
        entries.push((0x9003, 2, cnt, 0, Some(b))); // DateTimeOriginal (0x9003)
    }
    // ComponentsConfiguration (0x9101): required by the JPEG/EXIF spec. UNDEFINED[4] = Y,Cb,Cr,- (1,2,3,0).
    entries.push((0x9101, 7, 4, u32::from_le_bytes([1, 2, 3, 0]), None));
    if e.subject_distance_m > 0.0 {
        entries.push((0x9206, 5, 1, 0, Some(ratbytes(e.subject_distance_m)))); // SubjectDistance
    }
    if e.focal_length_mm > 0.0 {
        entries.push((0x920A, 5, 1, 0, Some(ratbytes(e.focal_length_mm)))); // FocalLength
    }
    // FlashpixVersion (0xA000) + ColorSpace (0xA001): JPEG/EXIF-spec required. ColorSpace=0xFFFF
    // (Uncalibrated) since our pixels are Rec.2020 (tagged via the ICC profile), not sRGB.
    entries.push((0xA000, 7, 4, u32::from_le_bytes(*b"0100"), None));
    entries.push((0xA001, 3, 1, 0xFFFF, None)); // ColorSpace = Uncalibrated
    // ExifImageWidth/Height (0xA002/0xA003) LONG - JPEG/EXIF-spec required (the pixel dimensions).
    if e.image_width > 0 {
        entries.push((0xA002, 4, 1, e.image_width, None));
        entries.push((0xA003, 4, 1, e.image_height, None));
    }
    if e.focal_length_35mm > 0.0 {
        entries.push((0xA405, 3, 1, (e.focal_length_35mm.round() as u32).min(65535), None)); // FocalLengthIn35mmFilm
    }
    entries
}

// Build the GPS IFD entries (tag ids are in the GPS namespace, ascending). Lat/lon are 3 RATIONALs
// (deg, min, sec) with an N/S/E/W ref; altitude is 1 RATIONAL with a 0/1 ref (above/below sea level).
pub fn build_gps_entries(e: &ExifData) -> Vec<ExifEntry> {
    let mut entries: Vec<ExifEntry> = Vec::new();
    if !e.has_gps {
        return entries;
    }
    // deg/min/sec as three RATIONALs (sec scaled by 1000 for sub-second precision).
    let dms = |v: f64| -> Vec<u8> {
        let a = v.abs();
        let deg = a.floor();
        let min = ((a - deg) * 60.0).floor();
        let sec = (a - deg - min / 60.0) * 3600.0;
        let mut b = Vec::new();
        for (n, d) in [(deg as u32, 1u32), (min as u32, 1), ((sec * 1000.0).round() as u32, 1000)] {
            b.extend(n.to_le_bytes());
            b.extend(d.to_le_bytes());
        }
        b
    };
    // GPSVersionID (0x0000) BYTE[4] = 2,3,0,0 (inline).
    entries.push((0x0000, 1, 4, u32::from_le_bytes([2, 3, 0, 0]), None));
    // GPSLatitudeRef (0x0001) ASCII "N"/"S"; GPSLatitude (0x0002) RATIONAL[3].
    let lat_ref = if e.gps_lat >= 0.0 { b"N\0" } else { b"S\0" };
    entries.push((0x0001, 2, 2, u32::from_le_bytes([lat_ref[0], lat_ref[1], 0, 0]), None));
    entries.push((0x0002, 5, 3, 0, Some(dms(e.gps_lat))));
    // GPSLongitudeRef (0x0003) ASCII "E"/"W"; GPSLongitude (0x0004) RATIONAL[3].
    let lon_ref = if e.gps_lon >= 0.0 { b"E\0" } else { b"W\0" };
    entries.push((0x0003, 2, 2, u32::from_le_bytes([lon_ref[0], lon_ref[1], 0, 0]), None));
    entries.push((0x0004, 5, 3, 0, Some(dms(e.gps_lon))));
    // GPSAltitudeRef (0x0005) BYTE 0=above/1=below sea level (inline); GPSAltitude (0x0006) RATIONAL.
    entries.push((0x0005, 1, 1, if e.gps_alt < 0.0 { 1 } else { 0 }, None));
    let alt = e.gps_alt.abs();
    let mut altb = ((alt * 1000.0).round() as u32).to_le_bytes().to_vec();
    altb.extend(1000u32.to_le_bytes());
    entries.push((0x0006, 5, 1, 0, Some(altb)));
    entries
}

// Write an IFD (count + 12-byte entries + next-IFD pointer) into `out` at the current end, returning the
// list of (value_field_position, payload) that still need their out-of-line data appended + offset
// patched. `next_ifd` is the 4-byte next-IFD pointer value (0 = end of chain).
fn write_ifd(out: &mut Vec<u8>, entries: &[ExifEntry], next_ifd: u32) -> Vec<(usize, Vec<u8>)> {
    out.extend((entries.len() as u16).to_le_bytes());
    let mut patches = Vec::new();
    for (tag, typ, count, inline, payload) in entries {
        out.extend(tag.to_le_bytes());
        out.extend(typ.to_le_bytes());
        out.extend(count.to_le_bytes());
        let valpos = out.len();
        out.extend(inline.to_le_bytes());
        if let Some(bytes) = payload {
            patches.push((valpos, bytes.clone()));
        }
    }
    out.extend(next_ifd.to_le_bytes());
    patches
}

// Append each out-of-line payload (word-aligned) to `out`, patching its IFD value field to the offset.
// `base` is the offset that file/segment offsets are measured from (0 for a standalone TIFF block).
fn append_payloads(out: &mut Vec<u8>, patches: Vec<(usize, Vec<u8>)>, base: usize) {
    for (valpos, bytes) in patches {
        if (out.len() - base) % 2 != 0 {
            out.push(0);
        }
        let off = ((out.len() - base) as u32).to_le_bytes();
        out[valpos..valpos + 4].copy_from_slice(&off);
        out.extend(&bytes);
    }
}

/// Build a complete standalone little-endian TIFF/EXIF block: TIFF header -> IFD0 (with an ExifIFD
/// pointer) -> ExifIFD (the actual tags). Offsets are relative to the start of this block. Used for the
/// JPEG APP1 "Exif\0\0" segment and TIFF embedding. Returns empty if there's nothing to write.
pub fn build_exif_block(exif: &ExifData) -> Vec<u8> {
    let exif_entries = build_exif_entries(exif);
    let gps_entries = build_gps_entries(exif);
    if exif_entries.is_empty() && gps_entries.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<u8> = Vec::new();
    // TIFF header: II, 42, offset to IFD0 (=8).
    out.extend([0x49, 0x49, 0x2A, 0x00, 8, 0, 0, 0]);
    // IFD0: the JPEG/EXIF-spec required resolution tags (out-of-line RATIONALs / inline SHORTs) plus the
    // ExifIFD (0x8769) and GPS IFD (0x8825) pointers - all in strict ascending tag-id order. The pointer
    // value fields are patched once those sub-IFDs are written. 72/1 dpi, inch units, centered YCbCr.
    let res72 = || -> Vec<u8> { let mut b = 72u32.to_le_bytes().to_vec(); b.extend(1u32.to_le_bytes()); b };
    // 4 fixed IFD0 tags (XRes, YRes, ResUnit, YCbCrPos) + the ExifIFD/GPS pointers when present.
    let mut ifd0_count = 4u16;
    if !exif_entries.is_empty() { ifd0_count += 1; }
    if !gps_entries.is_empty() { ifd0_count += 1; }
    out.extend(ifd0_count.to_le_bytes());
    // XResolution (0x011A) / YResolution (0x011B) RATIONAL (out-of-line, patched).
    out.extend([0x1A, 0x01, 5, 0, 1, 0, 0, 0]);
    let xres_pos = out.len(); out.extend([0, 0, 0, 0]);
    out.extend([0x1B, 0x01, 5, 0, 1, 0, 0, 0]);
    let yres_pos = out.len(); out.extend([0, 0, 0, 0]);
    // ResolutionUnit (0x0128) SHORT = 2 (inch), inline.
    out.extend([0x28, 0x01, 3, 0, 1, 0, 0, 0, 2, 0, 0, 0]);
    // YCbCrPositioning (0x0213) SHORT = 1 (centered), inline.
    out.extend([0x13, 0x02, 3, 0, 1, 0, 0, 0, 1, 0, 0, 0]);
    let mut exif_ptr_pos = 0usize;
    let mut gps_ptr_pos = 0usize;
    if !exif_entries.is_empty() {
        out.extend([0x69, 0x87, 4, 0, 1, 0, 0, 0]); // 0x8769 ExifIFD pointer, LONG
        exif_ptr_pos = out.len();
        out.extend([0, 0, 0, 0]);
    }
    if !gps_entries.is_empty() {
        out.extend([0x25, 0x88, 4, 0, 1, 0, 0, 0]); // 0x8825 GPS IFD pointer, LONG
        gps_ptr_pos = out.len();
        out.extend([0, 0, 0, 0]);
    }
    out.extend([0, 0, 0, 0]); // IFD0 next-IFD pointer = 0
    // Append the resolution RATIONALs (word-aligned) and patch their offsets.
    if out.len() % 2 != 0 { out.push(0); }
    let xoff = (out.len() as u32).to_le_bytes(); out[xres_pos..xres_pos + 4].copy_from_slice(&xoff); out.extend(res72());
    let yoff = (out.len() as u32).to_le_bytes(); out[yres_pos..yres_pos + 4].copy_from_slice(&yoff); out.extend(res72());
    // ExifIFD.
    if !exif_entries.is_empty() {
        let off = (out.len() as u32).to_le_bytes();
        out[exif_ptr_pos..exif_ptr_pos + 4].copy_from_slice(&off);
        let patches = write_ifd(&mut out, &exif_entries, 0);
        append_payloads(&mut out, patches, 0);
    }
    // GPS IFD.
    if !gps_entries.is_empty() {
        if out.len() % 2 != 0 { out.push(0); }
        let off = (out.len() as u32).to_le_bytes();
        out[gps_ptr_pos..gps_ptr_pos + 4].copy_from_slice(&off);
        let patches = write_ifd(&mut out, &gps_entries, 0);
        append_payloads(&mut out, patches, 0);
    }
    out
}


pub fn initialize_raw_info() -> RawInfo {
    RawInfo {
        make: "".to_owned(),
        makeoffset: 0,
        makelen: 0,
        model: "".to_owned(),
        modeloffset: 0,
        modellen: 0,
        width: 0,
        height: 0,
        bitdepth: 255,
        bitdepthold: 0,
        rgb: false,
        cfa: Vec::new(),
        cfaw: 0,
        cfah: 0,
        black: 0.,
        blackoffset: 0,
        blackcount: 0,
        blacktype: 0,
        white: 65535.,
        orientation: 9,
        compression: false,
        cam2terminal9: [
            1., 0., 0., //
            0., 1., 0., //
            0., 0., 1., //
        ],
        magic9inv: [
            1, 0, 0, 0, 1, 0, 0, 0, //
            0, 0, 0, 0, 1, 0, 0, 0, //
            0, 0, 0, 0, 1, 0, 0, 0, //
            //
            0, 0, 0, 0, 1, 0, 0, 0, //
            1, 0, 0, 0, 1, 0, 0, 0, //
            0, 0, 0, 0, 1, 0, 0, 0, //
            //
            0, 0, 0, 0, 1, 0, 0, 0, //
            0, 0, 0, 0, 1, 0, 0, 0, //
            1, 0, 0, 0, 1, 0, 0, 0, //
        ],
        magicoffset: 0,
        profileoffset: 0,
        curveoffset: 0,
        imagedataoffset: 0,
        ifdoffset: 0,
        duck: false,
        save_scan: false,
        cfapatternoffset: 0,
        preview_jpeg: None,
        preview_dims: (0, 0),
        description: String::new(),
        descriptionoffset: 0,
        exif: ExifData::default(),
        exififdpointeroffset: 0,
    }
}
pub struct RawInfo {
    pub make: String,
    pub makeoffset: u32,
    pub makelen: u32,
    pub model: String, //don't forget to null terminate ASCII
    pub modeloffset: u32,
    pub modellen: u32,
    pub width: usize,
    pub height: usize,
    pub bitdepth: u8,
    pub bitdepthold: u8,
    pub compression: bool,
    pub rgb: bool,
    pub cfa: Vec<u8>,
    pub cfaw: u16,
    pub cfah: u16,
    pub black: f32,
    pub blackoffset: u32,
    pub blackcount: u32,
    pub blacktype: u16,
    pub white: f32,
    pub orientation: u16,
    pub cam2terminal9: [f32; 9],
    pub magic9inv: [u8; 8 * 9],
    pub magicoffset: u32,
    pub profileoffset: u32,
    pub curveoffset: u32,
    pub imagedataoffset: u32,
    pub ifdoffset: u32,
    pub duck: bool,
    pub save_scan: bool,
    /// Patched with the file offset of the out-of-line 16-byte (4x4 quad-Bayer) CFA pattern. Unused (0) for inline 2x2 patterns.
    pub cfapatternoffset: u32,
    /// Optional embedded preview JPEG. When Some, make_base_dng chains a second IFD (IFD1) describing it as a reduced-resolution JPEG image, so viewers/thumbnailers can show the photo without demosaicing the raw. None = no preview.
    pub preview_jpeg: Option<Vec<u8>>,
    /// The preview JPEG's pixel dimensions (width, height), needed for IFD1's ImageWidth/ImageLength. Only read when preview_jpeg is Some.
    pub preview_dims: (u32, u32),
    /// Human-readable exposure summary written to ImageDescription (TIFF tag 270). Carries per-frame and
    /// composite/effective exposure (the integrator knows frame_count, per-frame shutter and ISO at save).
    /// Empty = tag omitted. Structured exposure fields are deferred to the VSF rollout.
    pub description: String,
    pub descriptionoffset: u32,
    /// Structured EXIF values written into an EXIF sub-IFD (tag 0x8769). Default/zeros = no EXIF IFD.
    pub exif: ExifData,
    /// Patched with the file offset of the EXIF-IFD-pointer tag's value field (where the sub-IFD offset goes).
    pub exififdpointeroffset: u32,
}

pub fn make_base_dng(rawinfo: &mut RawInfo) -> Vec<u8> {
    let profilename = "Verichrome scene-relative IDT";
    let word = 2; //whatever word means in the tiff spec
    let mut basedng = vec![73u8, 73, 42, 0, 8, 0, 0, 0]; //header
    let mut numifd: u16 = 0;
    rawinfo.ifdoffset = basedng.len() as u32;
    basedng.extend([0, 0]); //IFD entries

    basedng.extend([254, 0, 4, 0, 1, 0, 0, 0, 0, 0, 0, 0]); //Subfile type, Full-resolution image
    numifd += 1;

    basedng.extend([0, 1, 4, 0, 1, 0, 0, 0]); //Image width
    let widthu8 = (rawinfo.width as u32).to_le_bytes();
    basedng.extend(widthu8);
    numifd += 1;

    basedng.extend([1, 1, 4, 0, 1, 0, 0, 0]); //Image height
    let heightu8 = (rawinfo.height as u32).to_le_bytes();
    basedng.extend(heightu8);
    numifd += 1;

    basedng.extend([2, 1, 3, 0, 1, 0, 0, 0, rawinfo.bitdepth, 0, 0, 0]); //Bits per sample
    numifd += 1;

    basedng.extend([3, 1, 3, 0, 1, 0, 0, 0, 1, 0, 0, 0]); //Compression
    numifd += 1;

    basedng.extend([6, 1, 3, 0, 1, 0, 0, 0, 35, 128, 0, 0]); //Photometric interpretation
    numifd += 1;

    // ImageDescription (tag 270 = 0x010E). ASCII, out-of-line like Make/Model. Ordering: sits between
    // Photometric (262) and Make (271), as TIFF requires ascending tag ids. Only emitted when set.
    if !rawinfo.description.is_empty() {
        basedng.extend([14, 1, 2, 0]); //ImageDescription
        let mut descstr = rawinfo.description.clone();
        descstr.push('\0');
        basedng.extend((descstr.len() as u32).to_le_bytes());
        rawinfo.descriptionoffset = basedng.len() as u32;
        basedng.extend([0; 4]);
        numifd += 1;
    }

    basedng.extend([15, 1, 2, 0]); //Make
    let mut makestr = rawinfo.make.clone();
    makestr.push_str("\0");
    basedng.extend((makestr.len() as u32).to_le_bytes());
    rawinfo.makeoffset = basedng.len() as u32;
    basedng.extend([0; 4]);
    numifd += 1;

    basedng.extend([16, 1, 2, 0]); //Model
    let mut modelstr = rawinfo.model.clone();
    modelstr.push_str("\0");
    basedng.extend((modelstr.len() as u32).to_le_bytes());
    rawinfo.modeloffset = basedng.len() as u32;
    basedng.extend([0; 4]);
    numifd += 1;

    basedng.extend([17, 1, 4, 0, 1, 0, 0, 0, 0, 0, 0, 0]); //Image data offset
    rawinfo.imagedataoffset = (basedng.len() - 4) as u32;
    numifd += 1;

    basedng.extend([18, 1, 3, 0, 1, 0, 0, 0]); //Orientation
    basedng.extend(rawinfo.orientation.to_le_bytes());
    basedng.extend([0, 0]);
    numifd += 1;

    basedng.extend([21, 1, 3, 0, 1, 0, 0, 0, 1, 0, 0, 0]); //Samples per pixel
    numifd += 1;

    basedng.extend([22, 1, 4, 0, 1, 0, 0, 0]); //Rows per strip
    let heightu8 = (rawinfo.height as u32).to_le_bytes();
    basedng.extend(heightu8);
    numifd += 1;

    basedng.extend([23, 1, 4, 0, 1, 0, 0, 0]); //RAW bytecount
    let bytecount = (((rawinfo.width * rawinfo.height * rawinfo.bitdepth as usize - 1) / 8 + 1)
        as u32)
        .to_le_bytes();
    basedng.extend(bytecount);
    numifd += 1;

    basedng.extend([26, 1, 5, 0, 1, 0, 0, 0, 0, 0, 0, 0]); //X resolution
    let xresoffset = basedng.len() - 4;
    numifd += 1;

    basedng.extend([27, 1, 5, 0, 1, 0, 0, 0, 0, 0, 0, 0]); //Y resolution
    let yresoffset = basedng.len() - 4;
    numifd += 1;

    basedng.extend([28, 1, 3, 0, 1, 0, 0, 0, 1, 0, 0, 0]); //Planar config
    numifd += 1;

    basedng.extend([40, 1, 3, 0, 1, 0, 0, 0, 1, 0, 0, 0]); //Resolution unit
    numifd += 1;

    // SubIFDs (0x014A=330, type 4=LONG): points at the embedded-preview sub-IFD (DNG-spec preview
    // location; exiftool reports it as PreviewImage). Emitted HERE so tag 330 keeps ascending order
    // (after ResolutionUnit 296, before CFARepeatPatternDim 33421) - it was previously appended at the
    // end of IFD0, which tripped exiftool's "SubIFD out of sequence" validation warning. Value (the
    // sub-IFD offset) is patched once that IFD is written. Absent when there's no preview.
    let mut subifd_ptr_pos = 0usize;
    if rawinfo.preview_jpeg.is_some() {
        basedng.extend([74, 1, 4, 0, 1, 0, 0, 0]); //SubIFDs
        subifd_ptr_pos = basedng.len();
        basedng.extend([0, 0, 0, 0]); // placeholder offset, patched after the sub-IFD is written
        numifd += 1;
    }

    // CFARepeatPatternDim (0x828D): the CFA tile size. 2x2 for a standard Bayer, 4x4 for a quad-Bayer (Tetracell) sensor where 2x2 clusters share a colour.
    let (cfa_dim, cfa_w_h): (u16, u8) = match rawinfo.cfa.len() {
        4 => (2, 2),
        16 => (4, 4),
        _ => panic!("Only 2x2 (4-byte) or 4x4 (16-byte) CFA patterns are supported!"),
    };
    let _ = cfa_dim;
    basedng.extend([141, 130, 3, 0, 2, 0, 0, 0, cfa_w_h, 0, cfa_w_h, 0]); //CFA pattern dimension
    numifd += 1;

    // CFAPattern (0x828E): the per-cell colour indices. A 4-byte (2x2) pattern fits inline in the IFD value field; a 16-byte (4x4 quad-Bayer) pattern does not, so it is written out-of-line and patched in at the end (offset recorded in rawinfo.cfapatternoffset).
    if rawinfo.cfa.len() == 4 {
        basedng.extend([142, 130, 1, 0, 4, 0, 0, 0]); //CFA pattern (inline)
        basedng.extend(rawinfo.cfa.clone());
    } else {
        basedng.extend([142, 130, 1, 0, 16, 0, 0, 0]); //CFA pattern (16 bytes, out-of-line)
                                                       // The value field (the offset) is the next 4 bytes we append; record its position so it can be patched once the pattern's file offset is known.
        rawinfo.cfapatternoffset = basedng.len() as u32;
        basedng.extend([0, 0, 0, 0]); // placeholder offset, patched below
    }
    numifd += 1;

    // EXIF IFD pointer (tag 0x8769 = 34665, type LONG). Value = offset to the EXIF sub-IFD, patched once
    // that IFD is written. Tag id 34665 sorts after CFAPattern (33422) and before DNG version (50706),
    // preserving ascending tag order. Only emitted when there's EXIF data to write.
    let have_exif = rawinfo.exif.exposure_time_s > 0.0
        || rawinfo.exif.iso > 0.0
        || rawinfo.exif.focal_length_mm > 0.0
        || !rawinfo.exif.datetime_original.is_empty();
    if have_exif {
        basedng.extend([105, 135, 4, 0, 1, 0, 0, 0]); // 0x8769, LONG, count 1
        rawinfo.exififdpointeroffset = basedng.len() as u32;
        basedng.extend([0, 0, 0, 0]); // placeholder offset, patched after the EXIF IFD is written
        numifd += 1;
    }

    basedng.extend([18, 198, 1, 0, 4, 0, 0, 0, 1, 5, 0, 0]); //DNG version
    numifd += 1;

    basedng.extend([19, 198, 1, 0, 4, 0, 0, 0, 1, 0, 0, 0]); //DNG backward version
    numifd += 1;

    basedng.extend([26, 198, 4, 0, 1, 0, 0, 0]); //Black level
    basedng.extend((rawinfo.black.round() as u32).to_le_bytes());
    //basedng.extend((255u32).to_le_bytes());
    numifd += 1;

    basedng.extend([29, 198, 4, 0, 1, 0, 0, 0]); //White level
    basedng.extend((rawinfo.white as u32).to_le_bytes());
    numifd += 1;

    basedng.extend([33, 198, 10, 0, 9, 0, 0, 0, 0, 0, 0, 0]); //Magic 9 inverse
    rawinfo.magicoffset = (basedng.len() - 4) as u32;
    numifd += 1;

    // basedng.extend([40, 198, 11, 0, 3, 0, 0, 0, 0, 0, 0, 0]); //Asshatneutral
    // let asshatoffset = basedng.len() - 4;
    // numifd += 1;

    basedng.extend([90, 198, 3, 0, 1, 0, 0, 0, 23, 0, 0, 0]); //Illuminant
    numifd += 1;

    basedng.extend([248, 198, 2, 0]); //Colour profile name
    basedng.extend((profilename.len() as u32).to_le_bytes());
    rawinfo.profileoffset = basedng.len() as u32;
    basedng.extend([0; 4]);
    numifd += 1;

    basedng.extend([252, 198, 11, 0, 4, 0, 0, 0, 0, 0, 0, 0]); //Profile curve
    rawinfo.curveoffset = (basedng.len() - 4) as u32;
    numifd += 1;

    basedng.extend([253, 198, 4, 0, 1, 0, 0, 0, 0, 0, 0, 0]); //Colour profile embedding permission
    numifd += 1;

    // (SubIFDs tag 0x014A was emitted earlier, in ascending-tag-id position; subifd_ptr_pos holds its
    // value-field offset for patching once the sub-IFD is written.)

    // Next-IFD pointer. 0 = no chained sibling IFD (the preview is a SubIFD child of IFD0, not a chained IFD1).
    basedng.extend([0, 0, 0, 0]);

    if basedng.len() % word != 0 {
        basedng.extend(vec![0u8; word - basedng.len() % word]);
    }

    let offset = (basedng.len() as u32).to_le_bytes(); //Append make
    basedng[rawinfo.makeoffset as usize] = offset[0];
    basedng[rawinfo.makeoffset as usize + 1] = offset[1];
    basedng[rawinfo.makeoffset as usize + 2] = offset[2];
    basedng[rawinfo.makeoffset as usize + 3] = offset[3];
    basedng.extend(makestr.as_bytes());

    // Append ImageDescription string + patch its offset (only emitted when the tag was written above).
    if rawinfo.descriptionoffset != 0 {
        if basedng.len() % word != 0 {
            basedng.extend(vec![0u8; word - basedng.len() % word]);
        }
        let offset = (basedng.len() as u32).to_le_bytes();
        basedng[rawinfo.descriptionoffset as usize] = offset[0];
        basedng[rawinfo.descriptionoffset as usize + 1] = offset[1];
        basedng[rawinfo.descriptionoffset as usize + 2] = offset[2];
        basedng[rawinfo.descriptionoffset as usize + 3] = offset[3];
        basedng.extend(rawinfo.description.as_bytes());
        basedng.push(0); // null terminator
    }

    if basedng.len() % word != 0 {
        basedng.extend(vec![0u8; word - basedng.len() % word]);
    }

    let offset = (basedng.len() as u32).to_le_bytes(); //Append model
    basedng[rawinfo.modeloffset as usize] = offset[0];
    basedng[rawinfo.modeloffset as usize + 1] = offset[1];
    basedng[rawinfo.modeloffset as usize + 2] = offset[2];
    basedng[rawinfo.modeloffset as usize + 3] = offset[3];
    basedng.extend(modelstr.as_bytes());

    basedng.extend([0, 0, 0, 0]);
    if basedng.len() % word != 0 {
        basedng.extend(vec![0u8; word - basedng.len() % word]);
    }

    let offset = (basedng.len() as u32).to_le_bytes(); //Append X resolution
    basedng[xresoffset] = offset[0];
    basedng[xresoffset + 1] = offset[1];
    basedng[xresoffset + 2] = offset[2];
    basedng[xresoffset + 3] = offset[3];
    basedng.extend([0, 0, 0, 1, 0, 0, 0, 1]);

    basedng.extend([0, 0, 0, 0]);
    if basedng.len() % word != 0 {
        basedng.extend(vec![0u8; word - basedng.len() % word]);
    }

    let offset = (basedng.len() as u32).to_le_bytes(); //Append Y resolution
    basedng[yresoffset] = offset[0];
    basedng[yresoffset + 1] = offset[1];
    basedng[yresoffset + 2] = offset[2];
    basedng[yresoffset + 3] = offset[3];
    basedng.extend([0, 0, 0, 1, 0, 0, 0, 1]);

    basedng.extend([0, 0, 0, 0]);
    if basedng.len() % word != 0 {
        basedng.extend(vec![0u8; word - basedng.len() % word]);
    }
    let offset = (basedng.len() as u32).to_le_bytes(); //Append empty magic 9
    basedng[rawinfo.magicoffset as usize] = offset[0];
    basedng[rawinfo.magicoffset as usize + 1] = offset[1];
    basedng[rawinfo.magicoffset as usize + 2] = offset[2];
    basedng[rawinfo.magicoffset as usize + 3] = offset[3];
    rawinfo.magicoffset = basedng.len() as u32;
    basedng.extend([0; 3 * 3 * 2 * 4]);

    basedng.extend([0, 0, 0, 0]);
    if basedng.len() % word != 0 {
        basedng.extend(vec![0u8; word - basedng.len() % word]);
    }

    // let offset = (basedng.len() as u32).to_le_bytes(); //Append asshatneutral
    // basedng[asshatoffset] = offset[0];
    // basedng[asshatoffset + 1] = offset[1];
    // basedng[asshatoffset + 2] = offset[2];
    // basedng[asshatoffset + 3] = offset[3];
    // basedng.extend([ 0, 0, 128, 63, 0, 0, 128, 63,0, 0, 128, 63, 0, 0, 128, 63,0, 0, 128, 63, 0, 0, 128, 63]);

    let offset = (basedng.len() as u32).to_le_bytes(); //Append colour profile name
    basedng[rawinfo.profileoffset as usize] = offset[0];
    basedng[rawinfo.profileoffset as usize + 1] = offset[1];
    basedng[rawinfo.profileoffset as usize + 2] = offset[2];
    basedng[rawinfo.profileoffset as usize + 3] = offset[3];
    basedng.extend(profilename.as_bytes());

    basedng.extend([0, 0, 0, 0]);
    if basedng.len() % word != 0 {
        basedng.extend(vec![0u8; word - basedng.len() % word]);
    }

    let offset = (basedng.len() as u32).to_le_bytes(); //Append tone curve
    basedng[rawinfo.curveoffset as usize] = offset[0];
    basedng[rawinfo.curveoffset as usize + 1] = offset[1];
    basedng[rawinfo.curveoffset as usize + 2] = offset[2];
    basedng[rawinfo.curveoffset as usize + 3] = offset[3];
    basedng.extend([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 63, 0, 0, 128, 63]);

    basedng.extend([0, 0, 0, 0]);
    if basedng.len() % word != 0 {
        basedng.extend(vec![0u8; word - basedng.len() % word]);
    }

    // Append the 16-byte quad-Bayer CFA pattern out-of-line and patch its offset (only when a 4x4 pattern was requested; the 2x2 case is inline and leaves cfapatternoffset == 0).
    if rawinfo.cfapatternoffset != 0 {
        let offset = (basedng.len() as u32).to_le_bytes();
        basedng[rawinfo.cfapatternoffset as usize] = offset[0];
        basedng[rawinfo.cfapatternoffset as usize + 1] = offset[1];
        basedng[rawinfo.cfapatternoffset as usize + 2] = offset[2];
        basedng[rawinfo.cfapatternoffset as usize + 3] = offset[3];
        basedng.extend(rawinfo.cfa.clone());
        if basedng.len() % word != 0 {
            basedng.extend(vec![0u8; word - basedng.len() % word]);
        }
    }

    // Embedded preview: append the JPEG, then a SubIFD describing it as a reduced-resolution JPEG
    // image, and patch IFD0's SubIFDs tag to point at it. Done BEFORE the raw-data-offset patch below
    // so the raw still appends at the very end (the integrator appends it after we return). Raw tools
    // read the SubIFD's JPEGInterchangeFormat as the preview without ever demosaicing the raw.
    if let Some(jpeg) = rawinfo.preview_jpeg.clone() {
        if basedng.len() % word != 0 {
            basedng.extend(vec![0u8; word - basedng.len() % word]);
        }
        let jpeg_offset = basedng.len() as u32;
        basedng.extend(&jpeg);
        if basedng.len() % word != 0 {
            basedng.extend(vec![0u8; word - basedng.len() % word]);
        }
        // The sub-IFD starts here; patch IFD0's SubIFDs tag value to this offset.
        let subifd_offset = basedng.len() as u32;
        basedng[subifd_ptr_pos..subifd_ptr_pos + 4].copy_from_slice(&subifd_offset.to_le_bytes());

        // Build the preview sub-IFD. TIFF tags (LE): tag(2) type(2) count(4) value/offset(4). Types: 3=SHORT, 4=LONG.
        let (pw, ph) = rawinfo.preview_dims;
        let entries: [(u16, u16, u32, u32); 8] = [
            (254, 4, 1, 1),                 // NewSubfileType = 1 (reduced-resolution / preview)
            (256, 4, 1, pw),                // ImageWidth
            (257, 4, 1, ph),                // ImageLength
            (258, 3, 1, 8),                 // BitsPerSample = 8
            (259, 3, 1, 7),                 // Compression = 7 (JPEG)
            (262, 3, 1, 6),                 // PhotometricInterpretation = 6 (YCbCr, JPEG)
            (513, 4, 1, jpeg_offset),       // JPEGInterchangeFormat (offset to JPEG)
            (514, 4, 1, jpeg.len() as u32), // JPEGInterchangeFormatLength
        ];
        basedng.extend((entries.len() as u16).to_le_bytes());
        for (tag, typ, count, val) in entries {
            basedng.extend(tag.to_le_bytes());
            basedng.extend(typ.to_le_bytes());
            basedng.extend(count.to_le_bytes());
            // For SHORT (type 3) the value sits in the low 2 bytes of the 4-byte field, LE.
            basedng.extend(val.to_le_bytes());
        }
        basedng.extend([0, 0, 0, 0]); // sub-IFD next-IFD pointer = 0 (end of chain)
    }

    // EXIF sub-IFD (pointed to by tag 0x8769). Built like the preview IFD above: word-align, patch the
    // pointer's value field to here, write count + entries (ascending tag id) + next-IFD=0. RATIONALs
    // and the ASCII datetime are written out-of-line AFTER the IFD body; their value fields hold offsets
    // we patch as we go. SHORT values sit inline in the low 2 bytes. The exif metadata is the composite.
    if rawinfo.exififdpointeroffset != 0 {
        if basedng.len() % word != 0 {
            basedng.extend(vec![0u8; word - basedng.len() % word]);
        }
        let exif_ifd_pos = (basedng.len() as u32).to_le_bytes();
        let p = rawinfo.exififdpointeroffset as usize;
        basedng[p..p + 4].copy_from_slice(&exif_ifd_pos);

        // Build the entry list. For RATIONAL/ASCII we reserve a value slot now and remember (slot_pos,
        // payload) to append + patch after the IFD body. SHORT values are inline.
        let e = &rawinfo.exif;
        // A rational from an f64 with a fixed denominator that preserves small values.
        let rat = |v: f64| -> (u32, u32) {
            if v <= 0.0 { (0, 1) } else if v >= 1.0 { ((v * 1000.0).round() as u32, 1000) } else { (1000, (1000.0 / v).round() as u32) } };

        // (tag, type, count, inline_value_or_0, optional out-of-line payload bytes)
        let mut entries: Vec<(u16, u16, u32, u32, Option<Vec<u8>>)> = Vec::new();
        // ExposureTime (0x829A) RATIONAL
        if e.exposure_time_s > 0.0 {
            let (n, d) = rat(e.exposure_time_s);
            let mut b = n.to_le_bytes().to_vec(); b.extend(d.to_le_bytes());
            entries.push((0x829A, 5, 1, 0, Some(b)));
        }
        // FNumber (0x829D) RATIONAL
        if e.f_number > 0.0 {
            let (n, d) = rat(e.f_number);
            let mut b = n.to_le_bytes().to_vec(); b.extend(d.to_le_bytes());
            entries.push((0x829D, 5, 1, 0, Some(b)));
        }
        // ISOSpeedRatings (0x8827) SHORT (inline)
        if e.iso > 0.0 {
            entries.push((0x8827, 3, 1, (e.iso.round() as u32).min(65535), None));
        }
        // ExifVersion (0x9000) UNDEFINED, "0230" (4 bytes, inline)
        entries.push((0x9000, 7, 4, u32::from_le_bytes(*b"0230"), None));
        // DateTimeOriginal (0x9003) ASCII (out-of-line, null-terminated)
        if !e.datetime_original.is_empty() {
            let mut b = e.datetime_original.clone().into_bytes(); b.push(0);
            let cnt = b.len() as u32;
            entries.push((0x9003, 2, cnt, 0, Some(b)));
        }
        // SubjectDistance (0x9206) RATIONAL
        if e.subject_distance_m > 0.0 {
            let (n, d) = rat(e.subject_distance_m);
            let mut b = n.to_le_bytes().to_vec(); b.extend(d.to_le_bytes());
            entries.push((0x9206, 5, 1, 0, Some(b)));
        }
        // FocalLength (0x920A) RATIONAL
        if e.focal_length_mm > 0.0 {
            let (n, d) = rat(e.focal_length_mm);
            let mut b = n.to_le_bytes().to_vec(); b.extend(d.to_le_bytes());
            entries.push((0x920A, 5, 1, 0, Some(b)));
        }
        // FocalLengthIn35mmFilm (0xA405) SHORT (inline)
        if e.focal_length_35mm > 0.0 {
            entries.push((0xA405, 3, 1, (e.focal_length_35mm.round() as u32).min(65535), None));
        }

        basedng.extend((entries.len() as u16).to_le_bytes());
        // Reserve the 12-byte entries; record where each out-of-line value field sits so we can patch it.
        let mut patches: Vec<(usize, Vec<u8>)> = Vec::new();
        for (tag, typ, count, inline, payload) in &entries {
            basedng.extend(tag.to_le_bytes());
            basedng.extend(typ.to_le_bytes());
            basedng.extend(count.to_le_bytes());
            let valpos = basedng.len();
            basedng.extend(inline.to_le_bytes());
            if let Some(bytes) = payload {
                patches.push((valpos, bytes.clone()));
            }
        }
        basedng.extend([0, 0, 0, 0]); // EXIF IFD next-IFD pointer = 0
        // Append each out-of-line payload (word-aligned) and patch its value field to the offset.
        for (valpos, bytes) in patches {
            if basedng.len() % word != 0 {
                basedng.extend(vec![0u8; word - basedng.len() % word]);
            }
            let off = (basedng.len() as u32).to_le_bytes();
            basedng[valpos..valpos + 4].copy_from_slice(&off);
            basedng.extend(&bytes);
        }
    }

    let offset = (basedng.len() as u32).to_le_bytes(); //Assign raw data offset
    basedng[rawinfo.imagedataoffset as usize] = offset[0];
    basedng[rawinfo.imagedataoffset as usize + 1] = offset[1];
    basedng[rawinfo.imagedataoffset as usize + 2] = offset[2];
    basedng[rawinfo.imagedataoffset as usize + 3] = offset[3];

    let numifdu8 = numifd.to_le_bytes();
    basedng[rawinfo.ifdoffset as usize] = numifdu8[0];
    basedng[rawinfo.ifdoffset as usize + 1] = numifdu8[1];

    for index in 0..rawinfo.magic9inv.len() {
        basedng[rawinfo.magicoffset as usize + index] = rawinfo.magic9inv[index];
    }

    basedng
}
