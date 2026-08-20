//! RGBA → I420 conversion, BT.709 limited range (the assumption HD players
//! make; the VP9 bitstream is tagged to match in `vp9.rs`).

pub struct I420Frame {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

/// Integer BT.709 limited-range coefficients, scaled by 256.
fn ycbcr(r: i32, g: i32, b: i32) -> (u8, i32, i32) {
    let y = ((47 * r + 157 * g + 16 * b + 128) >> 8) + 16;
    let cb = (-26 * r - 87 * g + 112 * b + 128) >> 8;
    let cr = (112 * r - 102 * g - 10 * b + 128) >> 8;
    (y.clamp(16, 235) as u8, cb, cr)
}

/// Converts straight RGBA to I420. Odd dimensions are handled by clamping
/// the 2x2 chroma window at the edges.
pub fn rgba_to_i420(width: u32, height: u32, rgba: &[u8]) -> I420Frame {
    let w = width as usize;
    let h = height as usize;
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let mut y_plane = vec![0u8; w * h];
    let mut u_plane = vec![0u8; cw * ch];
    let mut v_plane = vec![0u8; cw * ch];

    for cy in 0..ch {
        for cx in 0..cw {
            let mut sum_cb = 0i32;
            let mut sum_cr = 0i32;
            let mut n = 0i32;
            for dy in 0..2 {
                let py = (cy * 2 + dy).min(h - 1);
                for dx in 0..2 {
                    let px = (cx * 2 + dx).min(w - 1);
                    // Edge pixels repeat in the window; keeping the count at
                    // the true sample number keeps the average unbiased.
                    if (cy * 2 + dy) < h && (cx * 2 + dx) < w {
                        let o = (py * w + px) * 4;
                        let (r, g, b) = (rgba[o] as i32, rgba[o + 1] as i32, rgba[o + 2] as i32);
                        let (y, cb, cr) = ycbcr(r, g, b);
                        y_plane[py * w + px] = y;
                        sum_cb += cb;
                        sum_cr += cr;
                        n += 1;
                    }
                }
            }
            u_plane[cy * cw + cx] = (128 + sum_cb / n).clamp(16, 240) as u8;
            v_plane[cy * cw + cx] = (128 + sum_cr / n).clamp(16, 240) as u8;
        }
    }
    I420Frame { width, height, y: y_plane, u: u_plane, v: v_plane }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            v.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        v
    }

    #[test]
    fn black_maps_to_video_black() {
        let f = rgba_to_i420(4, 4, &solid(4, 4, [0, 0, 0]));
        assert!(f.y.iter().all(|&y| y == 16));
        assert!(f.u.iter().all(|&u| u == 128));
        assert!(f.v.iter().all(|&v| v == 128));
    }

    #[test]
    fn white_maps_to_video_white() {
        let f = rgba_to_i420(4, 4, &solid(4, 4, [255, 255, 255]));
        assert!(f.y.iter().all(|&y| (234..=235).contains(&y)), "{:?}", &f.y[..4]);
        assert!(f.u.iter().all(|&u| (127..=129).contains(&u)));
    }

    #[test]
    fn red_lands_in_the_right_quadrant() {
        let f = rgba_to_i420(4, 4, &solid(4, 4, [255, 0, 0]));
        // BT.709 red: high Cr, below-center Cb, Y around 63.
        assert!(f.v[0] > 200, "Cr {}", f.v[0]);
        assert!(f.u[0] < 110, "Cb {}", f.u[0]);
        assert!((55..=70).contains(&f.y[0]), "Y {}", f.y[0]);
    }

    #[test]
    fn odd_dimensions_round_up_chroma() {
        let f = rgba_to_i420(5, 3, &solid(5, 3, [10, 200, 30]));
        assert_eq!(f.y.len(), 15);
        assert_eq!(f.u.len(), 3 * 2);
        assert_eq!(f.v.len(), 3 * 2);
    }
}
