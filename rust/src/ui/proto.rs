use image::{GenericImageView, open};
use rand::Rng;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    // Load background image
    let background_dynamic = open("background.jpeg").unwrap();
    let (bg_width, bg_height) = background_dynamic.dimensions();
    let background = background_dynamic.to_rgb8().into_raw();

    let width = bg_width as usize;
    let height = bg_height as usize;

    let filename = PathBuf::from("button.tiff");
    let mut img = Vec::with_capacity(min * min);
    let min = width.min(height);

    let x_patches = 7;
    let y_patches = 6;
    let mut random_patch_colours: Vec<(f32, f32, f32)> = Vec::with_capacity(x_patches * y_patches);
    let mut rng = rand::rng();

    // Generate random colours for each patch
    for _ in 0..(x_patches * y_patches) {
        random_patch_colours.push((rng.random(), rng.random(), rng.random()))
    }

    for y in 0..min {
        for x in 0..min {
            let mut xf = x as f32 / min as f32;
            xf = xf * 2. - 1.;
            let mut yf = y as f32 / min as f32;
            yf = yf * 2. - 1.;
            let mut xsf = xf;
            xsf = xsf * 4.;
            let mut ysf = yf;
            ysf = ysf * 4.;
            let x_patch = (xsf + 3.5).floor() as isize as usize;
            let y_patch = (ysf + 3.).floor() as isize as usize;
            let mut bullseye = false;
            let patch_shading = if xsf.abs() < 3.5 && ysf.abs() < 3. {
                if xsf.abs() > 2.5 && ysf.abs() > 2. {
                    xsf = 1. - ((xsf - xsf.floor()) * 2. - 1.).abs();
                    ysf = ((ysf - ysf.floor()) * 2. - 1.).abs();
                    let sf = (((xsf * xsf + ysf * ysf).sqrt().ln().min(0.) * 13.).cos() - 1.) / -2.;
                    bullseye = true;
                    sf * 0.75
                } else {
                    xsf = 1. - ((xsf - xsf.floor()) * 2. - 1.).abs();
                    ysf = ((ysf - ysf.floor()) * 2. - 1.).abs();
                    xsf *= xsf;
                    ysf *= ysf;
                    xsf *= xsf;
                    ysf *= ysf;
                    let mut sf = xsf * xsf + ysf * ysf;
                    sf = sf * 3.;
                    sf = (1. - sf * sf).max(0.);
                    sf
                }
            } else {
                0.
            };

            xf *= xf;
            yf *= yf * 4. / 3.; // aspect
            xf *= xf;
            yf *= yf;
            xf *= xf;
            yf *= yf;
            xf *= xf;
            yf *= yf;

            let mut value_f = xf + yf;
            value_f *= value_f * 4.;
            value_f *= value_f;

            let alpha = (value_f - 1.).min(1.).max(0.);
            value_f = 1. - value_f;

            value_f = 1. - value_f * value_f + patch_shading;
            let colour_idx = y_patch * x_patches + x_patch;
            let (r, g, b) = if colour_idx < random_patch_colours.len() && x_patch < x_patches {
                let colour = random_patch_colours[colour_idx];
                if bullseye {
                    (value_f, value_f, value_f)
                } else {
                    (
                        value_f * (colour.0 * 0.25 + 0.5),
                        value_f * (colour.1 * 0.25 + 0.5),
                        value_f * (colour.2 * 0.25 + 0.5),
                    )
                }
            } else if bullseye {
                (0., 0., 0.)
            } else {
                (value_f * 0.5, value_f * 0.5, value_f * 0.5)
            };

            let idx = (y * width + x) * 3;
            let bkg_r = background[idx];
            let bkg_g = background[idx + 1];
            let bkg_b = background[idx + 2];
            img.push((r.max(0.) * 256. + bkg_r as f32 * alpha) as u8);
            img.push((g.max(0.) * 256. + bkg_g as f32 * alpha) as u8);
            img.push((b.max(0.) * 256. + bkg_b as f32 * alpha) as u8);

            // img.push((r * 256.) as u8);
            // img.push((g * 256.) as u8);
            // img.push((b * 256.) as u8);
        }
    }
    make_tiff(&filename, min as u32, min as u32, img);
}

fn make_tiff(filename: &PathBuf, width: u32, height: u32, img: Vec<u8>) {
    let word = 2; //whatever word means in the tiff spec
    let mut basetiff = vec![73u8, 73, 42, 0, 8, 0, 0, 0]; //header
    let mut numifd: u16 = 0;
    let ifdoffset = basetiff.len() as u32;
    basetiff.extend([0, 0]); //IFD entries

    basetiff.extend([254, 0, 4, 0, 1, 0, 0, 0, 0, 0, 0, 0]); //Subfile type
    numifd += 1;

    basetiff.extend([0, 1, 4, 0, 1, 0, 0, 0]); //Image width
    let widthu8 = width.to_le_bytes();
    basetiff.extend(widthu8);
    numifd += 1;

    basetiff.extend([1, 1, 4, 0, 1, 0, 0, 0]); //Image height
    let heightu8 = height.to_le_bytes();
    basetiff.extend(heightu8);
    numifd += 1;

    // Bits per sample as SHORT[3] -> offset to values (filled later)
    basetiff.extend([2, 1, 3, 0, 3, 0, 0, 0, 0, 0, 0, 0]);
    let bits_per_sample_offset_index = basetiff.len() - 4;
    numifd += 1;

    basetiff.extend([3, 1, 3, 0, 1, 0, 0, 0, 1, 0, 0, 0]); //Compression
    numifd += 1;

    basetiff.extend([6, 1, 3, 0, 1, 0, 0, 0, 2, 0, 0, 0]); //Photometric interpretation
    numifd += 1;

    basetiff.extend([17, 1, 4, 0, 1, 0, 0, 0, 0, 0, 0, 0]); //Image data offset
    let imagedataoffset = (basetiff.len() - 4) as u32;
    numifd += 1;

    basetiff.extend([21, 1, 3, 0, 1, 0, 0, 0, 3, 0, 0, 0]); //Samples per pixel
    numifd += 1;

    basetiff.extend([22, 1, 4, 0, 1, 0, 0, 0]); //Rows per strip
    let heightu8 = height.to_le_bytes();
    basetiff.extend(heightu8);
    numifd += 1;

    basetiff.extend([23, 1, 4, 0, 1, 0, 0, 0]); //RAW bytecount
    let bytecount = (width * height * 3).to_le_bytes();
    basetiff.extend(bytecount);
    numifd += 1;

    basetiff.extend([26, 1, 1, 0, 1, 0, 0, 0, 1, 0, 0, 0]); //X resolution

    numifd += 1;

    basetiff.extend([27, 1, 1, 0, 1, 0, 0, 0, 1, 0, 0, 0]); //Y resolution

    numifd += 1;

    basetiff.extend([28, 1, 3, 0, 1, 0, 0, 0, 1, 0, 0, 0]); //Planar configuration

    numifd += 1;

    basetiff.extend([40, 1, 3, 0, 1, 0, 0, 0, 3, 0, 0, 0]); //Resolution unit
    numifd += 1;

    basetiff.extend([0, 0, 0, 0]); //disable chaining

    if basetiff.len() % word != 0 {
        basetiff.extend(vec![0u8; word - basetiff.len() % word]);
    }

    // Write BitsPerSample values (three SHORTs 8,8,8) and backfill the offset
    let bps_values_offset = basetiff.len() as u32;
    basetiff.extend([8u8, 0, 8, 0, 8, 0]);
    if basetiff.len() % word != 0 {
        basetiff.extend(vec![0u8; word - basetiff.len() % word]);
    }
    let bps_offset_le = bps_values_offset.to_le_bytes();
    basetiff[bits_per_sample_offset_index] = bps_offset_le[0];
    basetiff[bits_per_sample_offset_index + 1] = bps_offset_le[1];
    basetiff[bits_per_sample_offset_index + 2] = bps_offset_le[2];
    basetiff[bits_per_sample_offset_index + 3] = bps_offset_le[3];

    let offset = (basetiff.len() as u32).to_le_bytes(); //Assign raw data offset
    basetiff[imagedataoffset as usize] = offset[0];
    basetiff[imagedataoffset as usize + 1] = offset[1];
    basetiff[imagedataoffset as usize + 2] = offset[2];
    basetiff[imagedataoffset as usize + 3] = offset[3];

    let numifdu8 = numifd.to_le_bytes();
    basetiff[ifdoffset as usize] = numifdu8[0];
    basetiff[ifdoffset as usize + 1] = numifdu8[1];

    basetiff.extend(img);

    let mut tempname = filename.clone();
    tempname.set_extension("tif-");
    let mut file = File::create(&tempname).unwrap();
    file.write_all(&basetiff).unwrap();
    file.sync_all().unwrap();
    std::fs::rename(&tempname, &filename).unwrap();
}
