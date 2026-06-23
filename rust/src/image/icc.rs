//! Rec.2020 ICC profile + per-container embedding (JPEG / TIFF / JPEG XL).
//!
//! The RGB exports are encoded in Rec.2020 primaries with a sqrt (gamma 2.0) transfer (the same
//! pipeline as the on-screen BT.2020 surface). Without an embedded profile, viewers assume sRGB
//! and render the wide-gamut data wrong. We synthesize a compact ICC v2 matrix/TRC profile
//! describing BT.2020 primaries + gamma 2.0, and inject it into each container.
//!
//! The profile is built once and cached. Colour values are s15Fixed16 (ICC fixed-point).

use std::sync::OnceLock;

fn s15f16(x: f64) -> [u8; 4] {
    let v = (x * 65536.0).round() as i32;
    v.to_be_bytes()
}

/// XYZType tag body: 8-byte header ('XYZ ' + 0) + one XYZNumber (3 s15Fixed16).
fn xyz_tag(x: f64, y: f64, z: f64) -> Vec<u8> {
    let mut v = Vec::with_capacity(20);
    v.extend_from_slice(b"XYZ \0\0\0\0");
    v.extend_from_slice(&s15f16(x));
    v.extend_from_slice(&s15f16(y));
    v.extend_from_slice(&s15f16(z));
    v
}

/// curveType with a single gamma value (u8Fixed8Number). gamma 2.0 -> 0x0200.
fn curv_gamma2() -> Vec<u8> {
    let mut v = Vec::with_capacity(12);
    v.extend_from_slice(b"curv\0\0\0\0"); // sig + reserved
    v.extend_from_slice(&1u32.to_be_bytes()); // count = 1 -> gamma form
    v.extend_from_slice(&0x0200u16.to_be_bytes()); // gamma 2.0 in u8Fixed8
    v
}

/// textType (ICC v2) holding an ASCII description.
fn text_tag(s: &str) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"text\0\0\0\0");
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    v
}

/// Build a minimal valid ICC v2 RGB matrix/TRC profile for BT.2020 + gamma 2.0.
///
/// rXYZ/gXYZ/bXYZ are the BT.2020 primaries' XYZ contributions adapted to the D50 white point
/// (the ICC PCS white). These were computed offline (Bradford adaptation of the BT.2020 RGB->XYZ
/// matrix from D65 to D50) and are baked in as constants.
fn build_profile() -> Vec<u8> {
    // BT.2020 RGB->XYZ columns, Bradford-adapted to D50 (ICC PCS white).
    let r_xyz = (0.673459, 0.279033, -0.001924);
    let g_xyz = (0.165661, 0.675338, 0.029959);
    let b_xyz = (0.125100, 0.045629, 0.797178);
    // D50 white point (ICC standard PCS illuminant).
    let wtpt = (0.964203, 1.000000, 0.824905);

    // Tags to emit (signature, body). Order matters only for the tag table, not semantics.
    let tags: [(&[u8; 4], Vec<u8>); 8] = [
        (b"desc", text_tag("Lumis Rec.2020 gamma 2.0")),
        (b"wtpt", xyz_tag(wtpt.0, wtpt.1, wtpt.2)),
        (b"rXYZ", xyz_tag(r_xyz.0, r_xyz.1, r_xyz.2)),
        (b"gXYZ", xyz_tag(g_xyz.0, g_xyz.1, g_xyz.2)),
        (b"bXYZ", xyz_tag(b_xyz.0, b_xyz.1, b_xyz.2)),
        (b"rTRC", curv_gamma2()),
        (b"gTRC", curv_gamma2()),
        (b"bTRC", curv_gamma2()),
    ];
    // cprt is required; add it too.
    let mut tag_list: Vec<(&[u8; 4], Vec<u8>)> = Vec::new();
    tag_list.push((b"cprt", text_tag("Public Domain")));
    tag_list.extend(tags);

    let header_size = 128usize;
    let tag_count = tag_list.len();
    let tag_table_size = 4 + tag_count * 12;
    let mut data_offset = header_size + tag_table_size;
    // 4-byte align each tag's data.
    let mut tag_entries: Vec<(&[u8; 4], u32, u32)> = Vec::new();
    let mut data_blob: Vec<u8> = Vec::new();
    for (sig, body) in &tag_list {
        let off = data_offset as u32;
        tag_entries.push((sig, off, body.len() as u32));
        data_blob.extend_from_slice(body);
        // pad to 4-byte boundary
        while data_blob.len() % 4 != 0 {
            data_blob.push(0);
        }
        data_offset = header_size + tag_table_size + data_blob.len();
    }

    let total = header_size + tag_table_size + data_blob.len();
    let mut p = vec![0u8; header_size];
    // Profile size
    p[0..4].copy_from_slice(&(total as u32).to_be_bytes());
    // Preferred CMM: none. Version 2.4.0.
    p[8..12].copy_from_slice(&[0x02, 0x40, 0x00, 0x00]);
    p[12..16].copy_from_slice(b"mntr"); // device class: display
    p[16..20].copy_from_slice(b"RGB "); // data colour space
    p[20..24].copy_from_slice(b"XYZ "); // PCS
    // date/time (zeros ok), 'acsp' signature
    p[36..40].copy_from_slice(b"acsp");
    // PCS illuminant = D50 (required in header).
    p[68..72].copy_from_slice(&s15f16(0.964203));
    p[72..76].copy_from_slice(&s15f16(1.000000));
    p[76..80].copy_from_slice(&s15f16(0.824905));

    // Tag table.
    p.extend_from_slice(&(tag_count as u32).to_be_bytes());
    for (sig, off, len) in &tag_entries {
        p.extend_from_slice(*sig);
        p.extend_from_slice(&off.to_be_bytes());
        p.extend_from_slice(&len.to_be_bytes());
    }
    p.extend_from_slice(&data_blob);
    p
}

/// The cached Rec.2020 ICC profile bytes.
pub fn rec2020_icc() -> &'static [u8] {
    static PROFILE: OnceLock<Vec<u8>> = OnceLock::new();
    PROFILE.get_or_init(build_profile)
}

/// Insert an ICC profile into a baseline JPEG as one APP2 "ICC_PROFILE" segment.
///
/// The profile is split into <=65519-byte chunks (we expect a single chunk for our tiny profile).
/// The APP2 segment goes right after SOI (the first 2 bytes). Returns the new JPEG bytes.
pub fn jpeg_with_icc(jpeg: &[u8], icc: &[u8]) -> Vec<u8> {
    const MARKER: &[u8] = b"ICC_PROFILE\0";
    const MAX_CHUNK: usize = 65535 - 2 - MARKER.len() - 2; // length field + marker + 2 seq bytes
    let chunks: Vec<&[u8]> = if icc.is_empty() {
        vec![]
    } else {
        icc.chunks(MAX_CHUNK).collect()
    };
    let n = chunks.len() as u8;

    let mut out = Vec::with_capacity(jpeg.len() + icc.len() + 64);
    // SOI
    out.extend_from_slice(&jpeg[0..2]);
    for (i, chunk) in chunks.iter().enumerate() {
        let seg_len = 2 + MARKER.len() + 2 + chunk.len(); // length bytes + marker + seq/total + data
        out.extend_from_slice(&[0xFF, 0xE2]); // APP2
        out.extend_from_slice(&(seg_len as u16).to_be_bytes());
        out.extend_from_slice(MARKER);
        out.push((i + 1) as u8); // sequence number (1-based)
        out.push(n); // total chunks
        out.extend_from_slice(chunk);
    }
    // Rest of the original JPEG after SOI.
    out.extend_from_slice(&jpeg[2..]);
    out
}

// Note: JPEG XL colour is signalled *inside* the codestream, not via a container box. Rather
// than wrap a JP2-style 'colr' box (which JXL decoders ignore), the Lumis fork of zune-jpegxl
// writes a real Rec.2020 ColourEncoding into the codestream — see `JxlSimpleEncoder::set_rec2020`.
// So JXL needs no post-hoc ICC injection here.
