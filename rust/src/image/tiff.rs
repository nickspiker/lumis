
fn save_tiff(buffer: &FrameBuffer, path: &PathBuf, rotation: u16) -> bool {
    // Bin 2x2 to half resolution
    let half_width = buffer.width / 2;
    let half_height = buffer.height / 2;

    // Determine output dimensions based on rotation
    let (output_width, output_height) = match rotation {
        90 | 270 => (half_height, half_width),
        _ => (half_width, half_height),
    };

    let mut tiff_data = Vec::with_capacity(output_width * output_height * 3);

    // Apply rotation while binning
    for out_y in 0..output_height {
        for out_x in 0..output_width {
            // Map output coordinates to input coordinates based on rotation
            let (in_x, in_y) = match rotation {
                90 => (out_y, output_width - 1 - out_x), // Rotate 90° CCW
                180 => (output_width - 1 - out_x, output_height - 1 - out_y), // Rotate 180°
                270 => (output_height - 1 - out_y, out_x), // Rotate 90° CW
                _ => (out_x, out_y),                     // No rotation
            };

            // Map back to full resolution coordinates
            let sx = in_x * 2;
            let sy = in_y * 2;

            // Read 2x2 bayer block from integrated image
            let mut tl = buffer.integrated_image[sy * buffer.width + sx] as u32;
            let mut tr = buffer.integrated_image[sy * buffer.width + sx + 1] as u32;
            let mut bl = buffer.integrated_image[(sy + 1) * buffer.width + sx] as u32;
            let mut br = buffer.integrated_image[(sy + 1) * buffer.width + sx + 1] as u32;

            // Subtract black level
            let black = buffer.image_black_level as u32;
            tl = tl.saturating_sub(black);
            tr = tr.saturating_sub(black);
            bl = bl.saturating_sub(black);
            br = br.saturating_sub(black);

            // Demosaic based on bayer pattern
            let pixels = [tl, tr, bl, br];
            let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);

            for i in 0..4 {
                match buffer.bayer_pattern[i] {
                    0 => r = pixels[i],
                    1 => g += pixels[i],
                    2 => b = pixels[i],
                    _ => {}
                }
            }

            // Apply gamma 2 (square root) and push to 16-bit
            tiff_data.push(isqrt32(r * 65536));
            tiff_data.push(isqrt32(g * 32768));
            tiff_data.push(isqrt32(b * 65536));
        }
    }

    // Create TIFF file with rotated dimensions
    make_tiff(path, output_width as u32, output_height as u32, tiff_data)
}

fn make_tiff(filename: &PathBuf, width: u32, height: u32, img: Vec<u16>) -> bool {
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

    basetiff.extend([2, 1, 1, 0, 3, 0, 0, 0, 16, 16, 16, 0]); //Bits per sample
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
    let bytecount = (width * height * 6).to_le_bytes();
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

    let offset = (basetiff.len() as u32).to_le_bytes(); //Assign raw data offset
    basetiff[imagedataoffset as usize] = offset[0];
    basetiff[imagedataoffset as usize + 1] = offset[1];
    basetiff[imagedataoffset as usize + 2] = offset[2];
    basetiff[imagedataoffset as usize + 3] = offset[3];

    let numifdu8 = numifd.to_le_bytes();
    basetiff[ifdoffset as usize] = numifdu8[0];
    basetiff[ifdoffset as usize + 1] = numifdu8[1];

    let mut imgu8 = vec![0; img.len() * 2];
    LittleEndian::write_u16_into(&img, &mut imgu8);
    basetiff.extend(imgu8);

    write_tiff(basetiff, filename)
}

fn write_tiff(img: Vec<u8>, filename: &PathBuf) -> bool {
    let mut tempname = filename.clone();
    tempname.set_extension("tif-");

    let result = (|| -> Result<(), std::io::Error> {
        debug!("Creating temp file: {:?}", tempname);
        let mut file = File::create(&tempname)?;

        debug!("Writing {} bytes to TIFF", img.len());
        file.write_all(&img)?;

        debug!("Syncing file to disk");
        file.sync_all()?;

        debug!("Renaming {:?} to {:?}", tempname, filename);
        std::fs::rename(&tempname, &filename)?;

        Ok(())
    })();

    match result {
        Ok(_) => {
            info!("Successfully wrote TIFF to {:?}", filename);
            true
        }
        Err(e) => {
            error!("Failed to write TIFF to {:?}: {}", filename, e);
            // Try to clean up temp file if it exists
            if tempname.exists() {
                let _ = std::fs::remove_file(&tempname);
            }
            false
        }
    }
}
