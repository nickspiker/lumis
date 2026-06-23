use std::panic;

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

    basedng.extend([0, 0, 0, 0]); //disable chaining

    if basedng.len() % word != 0 {
        basedng.extend(vec![0u8; word - basedng.len() % word]);
    }

    let offset = (basedng.len() as u32).to_le_bytes(); //Append make
    basedng[rawinfo.makeoffset as usize] = offset[0];
    basedng[rawinfo.makeoffset as usize + 1] = offset[1];
    basedng[rawinfo.makeoffset as usize + 2] = offset[2];
    basedng[rawinfo.makeoffset as usize + 3] = offset[3];
    basedng.extend(makestr.as_bytes());

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
