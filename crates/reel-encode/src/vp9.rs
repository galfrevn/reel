//! Safe wrapper around libvpx's VP9 encoder (via `env-libvpx-sys`).
//! Compiled only with the `video` feature; everything else in this crate is
//! pure Rust.

use crate::webm::Block;
use crate::yuv::I420Frame;
use crate::EncodeError;
use std::mem::MaybeUninit;
use vpx_sys::*;

fn vpx_err(what: &str, code: vpx_codec_err_t) -> EncodeError {
    EncodeError::Vpx(format!("{what}: {code:?}"))
}

pub struct Vp9Config {
    /// Constrained-quality level, 0 (best) to 63 (worst).
    pub cq_level: u32,
    /// Bitrate cap in kbit/s.
    pub bitrate_kbps: u32,
    /// libvpx speed/quality dial (0 slowest..9 fastest).
    pub cpu_used: i32,
}

pub struct Vp9Encoder {
    ctx: vpx_codec_ctx_t,
    /// Reused input image — vpx_img_alloc/free per frame showed up in
    /// profiles on long renders.
    img: vpx_image_t,
    width: u32,
    height: u32,
}

// vpx_codec_ctx_t owns only heap state guarded by libvpx; we never share it.
unsafe impl Send for Vp9Encoder {}

impl Vp9Encoder {
    pub fn new(width: u32, height: u32, cfg: &Vp9Config) -> Result<Self, EncodeError> {
        unsafe {
            let iface = vpx_codec_vp9_cx();
            let mut enc_cfg = MaybeUninit::<vpx_codec_enc_cfg_t>::uninit();
            let rc = vpx_codec_enc_config_default(iface, enc_cfg.as_mut_ptr(), 0);
            if rc != VPX_CODEC_OK {
                return Err(vpx_err("config_default", rc));
            }
            let mut enc_cfg = enc_cfg.assume_init();
            enc_cfg.g_w = width;
            enc_cfg.g_h = height;
            // Millisecond timebase: pts values are our output-clock ms.
            enc_cfg.g_timebase.num = 1;
            enc_cfg.g_timebase.den = 1000;
            enc_cfg.rc_target_bitrate = cfg.bitrate_kbps;
            enc_cfg.rc_end_usage = vpx_rc_mode::VPX_CQ;
            // No lookahead: frames come out as they go in, which keeps
            // memory flat and muxing trivial.
            enc_cfg.g_lag_in_frames = 0;
            enc_cfg.g_threads = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
            enc_cfg.kf_max_dist = 300;

            let mut ctx = MaybeUninit::<vpx_codec_ctx_t>::uninit();
            let rc = vpx_codec_enc_init_ver(
                ctx.as_mut_ptr(),
                iface,
                &enc_cfg,
                0,
                VPX_ENCODER_ABI_VERSION as i32,
            );
            if rc != VPX_CODEC_OK {
                return Err(vpx_err("enc_init", rc));
            }
            let mut ctx = ctx.assume_init();

            let controls = [
                (vp8e_enc_control_id::VP8E_SET_CQ_LEVEL as i32, cfg.cq_level as i32),
                (vp8e_enc_control_id::VP8E_SET_CPUUSED as i32, cfg.cpu_used),
                // Terminal output is the textbook screen-content case.
                (vp8e_enc_control_id::VP9E_SET_TUNE_CONTENT as i32, vp9e_tune_content::VP9E_CONTENT_SCREEN as i32),
                (vp8e_enc_control_id::VP9E_SET_ROW_MT as i32, 1),
                // Tag the bitstream to match yuv.rs's BT.709 conversion.
                (vp8e_enc_control_id::VP9E_SET_COLOR_SPACE as i32, vpx_color_space::VPX_CS_BT_709 as i32),
            ];
            for (id, val) in controls {
                let rc = vpx_codec_control_(&mut ctx, id, val);
                if rc != VPX_CODEC_OK {
                    vpx_codec_destroy(&mut ctx);
                    return Err(vpx_err("codec_control", rc));
                }
            }

            let mut img = MaybeUninit::<vpx_image_t>::uninit();
            if vpx_img_alloc(
                img.as_mut_ptr(),
                vpx_img_fmt::VPX_IMG_FMT_I420,
                width,
                height,
                16,
            )
            .is_null()
            {
                vpx_codec_destroy(&mut ctx);
                return Err(EncodeError::Vpx("vpx_img_alloc failed".into()));
            }
            Ok(Vp9Encoder { ctx, img: img.assume_init(), width, height })
        }
    }

    /// Encodes one frame; returns any packets libvpx emits (with lag 0,
    /// normally exactly one per call).
    pub fn encode(
        &mut self,
        frame: &I420Frame,
        pts_ms: i64,
        dur_ms: u64,
    ) -> Result<Vec<Block>, EncodeError> {
        assert_eq!((frame.width, frame.height), (self.width, self.height));
        unsafe {
            let img = &mut self.img;
            copy_plane(img.planes[0], img.stride[0] as usize, &frame.y, self.width as usize, self.height as usize);
            let cw = (self.width as usize).div_ceil(2);
            let ch = (self.height as usize).div_ceil(2);
            copy_plane(img.planes[1], img.stride[1] as usize, &frame.u, cw, ch);
            copy_plane(img.planes[2], img.stride[2] as usize, &frame.v, cw, ch);

            let rc = vpx_codec_encode(
                &mut self.ctx,
                &self.img,
                pts_ms,
                dur_ms.max(1),
                0,
                VPX_DL_GOOD_QUALITY as u64,
            );
            if rc != VPX_CODEC_OK {
                return Err(vpx_err("encode", rc));
            }
            Ok(self.drain())
        }
    }

    /// Re-encodes the image already loaded by the previous [`encode`]
    /// (Self::encode) call — the CFR hold path re-emitting an unchanged
    /// still. Skips the plane copies and takes the realtime deadline: the
    /// encoder sees a zero-motion duplicate, so quality is irrelevant.
    pub fn encode_repeat(&mut self, pts_ms: i64, dur_ms: u64) -> Result<Vec<Block>, EncodeError> {
        unsafe {
            let rc = vpx_codec_encode(
                &mut self.ctx,
                &self.img,
                pts_ms,
                dur_ms.max(1),
                0,
                VPX_DL_REALTIME as u64,
            );
            if rc != VPX_CODEC_OK {
                return Err(vpx_err("encode", rc));
            }
            Ok(self.drain())
        }
    }

    /// Flushes the encoder; call once after the last frame.
    pub fn finish(&mut self) -> Result<Vec<Block>, EncodeError> {
        unsafe {
            let rc = vpx_codec_encode(
                &mut self.ctx,
                std::ptr::null(),
                -1,
                1,
                0,
                VPX_DL_GOOD_QUALITY as u64,
            );
            if rc != VPX_CODEC_OK {
                return Err(vpx_err("flush", rc));
            }
            Ok(self.drain())
        }
    }

    unsafe fn drain(&mut self) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut iter: vpx_codec_iter_t = std::ptr::null();
        loop {
            let pkt = vpx_codec_get_cx_data(&mut self.ctx, &mut iter);
            if pkt.is_null() {
                break;
            }
            if (*pkt).kind == vpx_codec_cx_pkt_kind::VPX_CODEC_CX_FRAME_PKT {
                let f = (*pkt).data.frame;
                let data = std::slice::from_raw_parts(f.buf as *const u8, f.sz).to_vec();
                blocks.push(Block {
                    pts_ms: f.pts,
                    track: 1,
                    keyframe: f.flags & VPX_FRAME_IS_KEY != 0,
                    data,
                    duration_ms: None,
                });
            }
        }
        blocks
    }
}

impl Drop for Vp9Encoder {
    fn drop(&mut self) {
        unsafe {
            vpx_img_free(&mut self.img);
            vpx_codec_destroy(&mut self.ctx);
        }
    }
}

unsafe fn copy_plane(dst: *mut u8, stride: usize, src: &[u8], w: usize, h: usize) {
    for row in 0..h {
        std::ptr::copy_nonoverlapping(src.as_ptr().add(row * w), dst.add(row * stride), w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yuv::rgba_to_i420;

    fn frame(w: u32, h: u32, seed: u8) -> I420Frame {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for i in 0..w * h {
            rgba.extend_from_slice(&[(i as u8).wrapping_add(seed), seed, 40, 255]);
        }
        rgba_to_i420(w, h, &rgba)
    }

    #[test]
    fn encodes_frames_and_marks_keyframe() {
        let cfg = Vp9Config { cq_level: 30, bitrate_kbps: 500, cpu_used: 8 };
        let mut enc = Vp9Encoder::new(64, 48, &cfg).unwrap();
        let mut blocks = Vec::new();
        for i in 0..5 {
            blocks.extend(enc.encode(&frame(64, 48, i * 30), i as i64 * 33, 33).unwrap());
        }
        blocks.extend(enc.finish().unwrap());
        assert_eq!(blocks.len(), 5);
        assert!(blocks[0].keyframe);
        assert!(blocks.iter().all(|b| !b.data.is_empty() && b.track == 1));
        assert_eq!(blocks[1].pts_ms, 33);
    }
}
