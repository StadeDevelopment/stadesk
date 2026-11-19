use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::os::raw::{c_int, c_uint};
use std::ptr;

use vpx_sys::{
    VPX_DECODER_ABI_VERSION, VPX_DL_REALTIME, VPX_EFLAG_FORCE_KF, VPX_ENCODER_ABI_VERSION,
    VPX_FRAME_IS_KEY, vp8e_enc_control_id, vp9e_tune_content, vpx_codec_ctx_t,
    vpx_codec_cx_pkt_kind, vpx_codec_dec_cfg_t, vpx_codec_dec_init_ver, vpx_codec_decode,
    vpx_codec_destroy, vpx_codec_enc_cfg_t, vpx_codec_enc_config_default, vpx_codec_enc_config_set,
    vpx_codec_enc_init_ver, vpx_codec_encode, vpx_codec_err_t, vpx_codec_err_to_string,
    vpx_codec_error_detail, vpx_codec_get_cx_data, vpx_codec_get_frame, vpx_codec_iter_t,
    vpx_codec_vp9_cx, vpx_codec_vp9_dx, vpx_color_range, vpx_color_space, vpx_enc_deadline_t,
    vpx_enc_frame_flags_t, vpx_image_t, vpx_img_fmt, vpx_img_wrap, vpx_kf_mode, vpx_rc_mode,
};

use crate::error::CodecError;
use crate::frame::{I420Frame, MAX_DIMENSION};

const MILLISECOND_TIMEBASE: c_int = 1000;
const CYCLIC_REFRESH_AQ: c_int = 3;
const MIN_TILE_WIDTH: u32 = 256;
const MAX_THREADS: u32 = 8;
const MAX_SPEED: i32 = 9;
const DEFAULT_SPEED: i32 = 8;
const DEFAULT_USAGE: c_uint = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderSettings {
    pub width: u32,
    pub height: u32,
    pub bitrate_kbps: u32,
    pub threads: u32,
    pub speed: i32,
}

impl EncoderSettings {
    pub fn new(width: u32, height: u32, bitrate_kbps: u32) -> Self {
        let threads = std::thread::available_parallelism()
            .map(|count| count.get() as u32)
            .unwrap_or(1)
            .clamp(1, MAX_THREADS);
        Self {
            width,
            height,
            bitrate_kbps,
            threads,
            speed: DEFAULT_SPEED,
        }
    }

    fn validate(&self) -> Result<(), CodecError> {
        if self.width == 0
            || self.height == 0
            || self.width > MAX_DIMENSION
            || self.height > MAX_DIMENSION
        {
            return Err(CodecError::InvalidDimensions {
                width: self.width,
                height: self.height,
            });
        }
        if self.bitrate_kbps == 0 {
            return Err(CodecError::InvalidSetting("bitrate must be positive"));
        }
        if self.threads == 0 || self.threads > MAX_THREADS {
            return Err(CodecError::InvalidSetting(
                "threads must be between 1 and 8",
            ));
        }
        if !(0..=MAX_SPEED).contains(&self.speed) {
            return Err(CodecError::InvalidSetting("speed must be between 0 and 9"));
        }
        Ok(())
    }

    fn tile_columns_log2(&self) -> c_int {
        let by_threads = self.threads.ilog2();
        let by_width = (self.width / MIN_TILE_WIDTH).max(1).ilog2();
        by_threads.min(by_width) as c_int
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub keyframe: bool,
    pub timestamp_ms: i64,
}

fn libvpx_result(
    status: vpx_codec_err_t,
    context: Option<&vpx_codec_ctx_t>,
    operation: &'static str,
) -> Result<(), CodecError> {
    if status == vpx_codec_err_t::VPX_CODEC_OK {
        return Ok(());
    }
    let mut message = unsafe { CStr::from_ptr(vpx_codec_err_to_string(status)) }
        .to_string_lossy()
        .into_owned();
    if let Some(context) = context {
        let detail = unsafe { vpx_codec_error_detail(context) };
        if !detail.is_null() {
            message.push_str(": ");
            message.push_str(&unsafe { CStr::from_ptr(detail) }.to_string_lossy());
        }
    }
    Err(CodecError::Libvpx { operation, message })
}

struct Context(Box<vpx_codec_ctx_t>);

impl Context {
    fn new() -> Self {
        Self(Box::new(unsafe {
            MaybeUninit::<vpx_codec_ctx_t>::zeroed().assume_init()
        }))
    }

    fn as_mut_ptr(&mut self) -> *mut vpx_codec_ctx_t {
        &mut *self.0
    }

    fn as_ref(&self) -> &vpx_codec_ctx_t {
        &self.0
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        if !self.0.priv_.is_null() {
            unsafe {
                vpx_codec_destroy(&mut *self.0);
            }
        }
    }
}

pub struct Vp9Encoder {
    context: Context,
    config: vpx_codec_enc_cfg_t,
    width: u32,
    height: u32,
    last_timestamp: Option<i64>,
}

unsafe impl Send for Vp9Encoder {}

impl Vp9Encoder {
    pub fn new(settings: EncoderSettings) -> Result<Self, CodecError> {
        settings.validate()?;
        let interface = unsafe { vpx_codec_vp9_cx() };
        let mut config = MaybeUninit::<vpx_codec_enc_cfg_t>::uninit();
        libvpx_result(
            unsafe { vpx_codec_enc_config_default(interface, config.as_mut_ptr(), DEFAULT_USAGE) },
            None,
            "encoder default configuration",
        )?;
        let mut config = unsafe { config.assume_init() };
        config.g_w = settings.width;
        config.g_h = settings.height;
        config.g_threads = settings.threads;
        config.g_timebase.num = 1;
        config.g_timebase.den = MILLISECOND_TIMEBASE;
        config.g_lag_in_frames = 0;
        config.g_error_resilient = 0;
        config.rc_end_usage = vpx_rc_mode::VPX_CBR;
        config.rc_target_bitrate = settings.bitrate_kbps;
        config.kf_mode = vpx_kf_mode::VPX_KF_FIXED;

        let mut encoder = Self {
            context: Context::new(),
            config,
            width: settings.width,
            height: settings.height,
            last_timestamp: None,
        };
        let status = unsafe {
            vpx_codec_enc_init_ver(
                encoder.context.as_mut_ptr(),
                interface,
                &encoder.config,
                0,
                VPX_ENCODER_ABI_VERSION as c_int,
            )
        };
        libvpx_result(
            status,
            Some(encoder.context.as_ref()),
            "encoder initialization",
        )?;
        encoder.control(vp8e_enc_control_id::VP8E_SET_CPUUSED, settings.speed)?;
        encoder.control(
            vp8e_enc_control_id::VP9E_SET_TUNE_CONTENT,
            vp9e_tune_content::VP9E_CONTENT_SCREEN as c_int,
        )?;
        encoder.control(vp8e_enc_control_id::VP9E_SET_AQ_MODE, CYCLIC_REFRESH_AQ)?;
        encoder.control(vp8e_enc_control_id::VP9E_SET_ROW_MT, 1)?;
        encoder.control(
            vp8e_enc_control_id::VP9E_SET_TILE_COLUMNS,
            settings.tile_columns_log2(),
        )?;
        Ok(encoder)
    }

    fn control(&mut self, id: vp8e_enc_control_id, value: c_int) -> Result<(), CodecError> {
        let status =
            unsafe { vpx_sys::vpx_codec_control_(self.context.as_mut_ptr(), id as c_int, value) };
        libvpx_result(status, Some(self.context.as_ref()), "encoder control")
    }

    pub fn set_bitrate(&mut self, bitrate_kbps: u32) -> Result<(), CodecError> {
        if bitrate_kbps == 0 {
            return Err(CodecError::InvalidSetting("bitrate must be positive"));
        }
        self.config.rc_target_bitrate = bitrate_kbps;
        let status = unsafe { vpx_codec_enc_config_set(self.context.as_mut_ptr(), &self.config) };
        libvpx_result(status, Some(self.context.as_ref()), "bitrate change")
    }

    pub fn encode(
        &mut self,
        frame: &I420Frame,
        timestamp_ms: i64,
        force_keyframe: bool,
    ) -> Result<Vec<EncodedFrame>, CodecError> {
        if frame.width() != self.width || frame.height() != self.height {
            return Err(CodecError::FrameSizeMismatch {
                expected_width: self.width,
                expected_height: self.height,
                actual_width: frame.width(),
                actual_height: frame.height(),
            });
        }
        let timestamp = match self.last_timestamp {
            Some(last) if timestamp_ms <= last => last + 1,
            _ => timestamp_ms,
        };
        let mut image = unsafe { MaybeUninit::<vpx_image_t>::zeroed().assume_init() };
        let base = frame.as_ptr().cast_mut();
        let wrapped = unsafe {
            vpx_img_wrap(
                &mut image,
                vpx_img_fmt::VPX_IMG_FMT_I420,
                self.width,
                self.height,
                1,
                base,
            )
        };
        if wrapped.is_null() {
            return Err(CodecError::Libvpx {
                operation: "image wrap",
                message: "vpx_img_wrap returned null".to_owned(),
            });
        }
        let luma_len = self.width as usize * self.height as usize;
        let chroma_width = frame.chroma_width();
        let chroma_len = chroma_width * frame.chroma_height();
        image.planes[0] = base;
        image.planes[1] = unsafe { base.add(luma_len) };
        image.planes[2] = unsafe { base.add(luma_len + chroma_len) };
        image.planes[3] = ptr::null_mut();
        image.stride[0] = self.width as c_int;
        image.stride[1] = chroma_width as c_int;
        image.stride[2] = chroma_width as c_int;
        image.stride[3] = 0;
        image.cs = vpx_color_space::VPX_CS_BT_601;
        image.range = vpx_color_range::VPX_CR_STUDIO_RANGE;

        let flags: vpx_enc_frame_flags_t = if force_keyframe {
            VPX_EFLAG_FORCE_KF as vpx_enc_frame_flags_t
        } else {
            0
        };
        let status = unsafe {
            vpx_codec_encode(
                self.context.as_mut_ptr(),
                &image,
                timestamp,
                1,
                flags,
                VPX_DL_REALTIME as vpx_enc_deadline_t,
            )
        };
        libvpx_result(status, Some(self.context.as_ref()), "encode")?;
        self.last_timestamp = Some(timestamp);

        let mut packets = Vec::new();
        let mut iterator: vpx_codec_iter_t = ptr::null();
        loop {
            let packet = unsafe { vpx_codec_get_cx_data(self.context.as_mut_ptr(), &mut iterator) };
            if packet.is_null() {
                break;
            }
            let packet = unsafe { &*packet };
            if packet.kind != vpx_codec_cx_pkt_kind::VPX_CODEC_CX_FRAME_PKT {
                continue;
            }
            let frame = unsafe { packet.data.frame };
            let data = if frame.sz == 0 || frame.buf.is_null() {
                Vec::new()
            } else {
                unsafe { std::slice::from_raw_parts(frame.buf as *const u8, frame.sz) }.to_vec()
            };
            packets.push(EncodedFrame {
                data,
                keyframe: frame.flags & VPX_FRAME_IS_KEY != 0,
                timestamp_ms: frame.pts,
            });
        }
        Ok(packets)
    }
}

pub struct Vp9Decoder {
    context: Context,
}

unsafe impl Send for Vp9Decoder {}

impl Vp9Decoder {
    pub fn new(threads: u32) -> Result<Self, CodecError> {
        if threads == 0 || threads > MAX_THREADS {
            return Err(CodecError::InvalidSetting(
                "threads must be between 1 and 8",
            ));
        }
        let config = vpx_codec_dec_cfg_t {
            threads,
            w: 0,
            h: 0,
        };
        let mut decoder = Self {
            context: Context::new(),
        };
        let status = unsafe {
            vpx_codec_dec_init_ver(
                decoder.context.as_mut_ptr(),
                vpx_codec_vp9_dx(),
                &config,
                0,
                VPX_DECODER_ABI_VERSION as c_int,
            )
        };
        libvpx_result(
            status,
            Some(decoder.context.as_ref()),
            "decoder initialization",
        )?;
        Ok(decoder)
    }

    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<I420Frame>, CodecError> {
        if data.is_empty() {
            return Err(CodecError::InvalidPacket);
        }
        let length = c_uint::try_from(data.len()).map_err(|_| CodecError::InvalidPacket)?;
        let status = unsafe {
            vpx_codec_decode(
                self.context.as_mut_ptr(),
                data.as_ptr(),
                length,
                ptr::null_mut(),
                0,
            )
        };
        libvpx_result(status, Some(self.context.as_ref()), "decode")?;

        let mut frames = Vec::new();
        let mut iterator: vpx_codec_iter_t = ptr::null();
        loop {
            let image = unsafe { vpx_codec_get_frame(self.context.as_mut_ptr(), &mut iterator) };
            if image.is_null() {
                break;
            }
            frames.push(copy_decoded_image(unsafe { &*image })?);
        }
        Ok(frames)
    }
}

fn copy_plane(
    source: *const u8,
    stride: c_int,
    width: usize,
    height: usize,
    destination: &mut [u8],
) -> Result<(), CodecError> {
    let stride = usize::try_from(stride).map_err(|_| CodecError::UnsupportedImage)?;
    if source.is_null() || stride < width {
        return Err(CodecError::UnsupportedImage);
    }
    for row in 0..height {
        let line = unsafe { std::slice::from_raw_parts(source.add(row * stride), width) };
        destination[row * width..(row + 1) * width].copy_from_slice(line);
    }
    Ok(())
}

fn copy_decoded_image(image: &vpx_image_t) -> Result<I420Frame, CodecError> {
    if image.fmt != vpx_img_fmt::VPX_IMG_FMT_I420
        || image.bit_depth != 8
        || image.x_chroma_shift != 1
        || image.y_chroma_shift != 1
    {
        return Err(CodecError::UnsupportedImage);
    }
    let mut frame = I420Frame::black(image.d_w, image.d_h)?;
    let width = image.d_w as usize;
    let height = image.d_h as usize;
    let chroma_width = frame.chroma_width();
    let chroma_height = frame.chroma_height();
    let (y, u, v) = frame.planes_mut();
    copy_plane(image.planes[0], image.stride[0], width, height, y)?;
    copy_plane(
        image.planes[1],
        image.stride[1],
        chroma_width,
        chroma_height,
        u,
    )?;
    copy_plane(
        image.planes[2],
        image.stride[2],
        chroma_width,
        chroma_height,
        v,
    )?;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::tests::gradient_bgra;

    fn frame(width: u32, height: u32, shift: u32) -> I420Frame {
        I420Frame::from_bgra(
            width,
            height,
            &gradient_bgra(width, height, shift),
            width as usize * 4,
        )
        .unwrap()
    }

    fn luma_psnr(original: &I420Frame, decoded: &I420Frame) -> f64 {
        let squared: f64 = original
            .y()
            .iter()
            .zip(decoded.y())
            .map(|(a, b)| {
                let difference = f64::from(*a) - f64::from(*b);
                difference * difference
            })
            .sum();
        let mean = squared / original.y().len() as f64;
        if mean == 0.0 {
            f64::INFINITY
        } else {
            10.0 * (255.0 * 255.0 / mean).log10()
        }
    }

    #[test]
    fn settings_validation_rejects_out_of_range_values() {
        let good = EncoderSettings::new(640, 480, 1000);
        assert!(good.validate().is_ok());
        for bad in [
            EncoderSettings { width: 0, ..good },
            EncoderSettings {
                height: MAX_DIMENSION + 1,
                ..good
            },
            EncoderSettings {
                bitrate_kbps: 0,
                ..good
            },
            EncoderSettings { threads: 0, ..good },
            EncoderSettings { threads: 9, ..good },
            EncoderSettings { speed: -1, ..good },
            EncoderSettings { speed: 10, ..good },
        ] {
            assert!(Vp9Encoder::new(bad).is_err());
        }
    }

    #[test]
    fn tile_columns_respect_thread_count_and_minimum_tile_width() {
        let settings = |width, threads| EncoderSettings {
            threads,
            ..EncoderSettings::new(width, 720, 1000)
        };
        assert_eq!(settings(1920, 8).tile_columns_log2(), 2);
        assert_eq!(settings(1920, 2).tile_columns_log2(), 1);
        assert_eq!(settings(320, 8).tile_columns_log2(), 0);
        assert_eq!(settings(3840, 8).tile_columns_log2(), 3);
    }

    #[test]
    fn encoded_stream_decodes_with_high_fidelity() {
        let (width, height) = (320, 240);
        let mut encoder = Vp9Encoder::new(EncoderSettings::new(width, height, 2000)).unwrap();
        let mut decoder = Vp9Decoder::new(2).unwrap();
        let mut decoded_count = 0;
        for index in 0..30u32 {
            let original = frame(width, height, index * 3);
            let packets = encoder
                .encode(&original, i64::from(index) * 33, false)
                .unwrap();
            for packet in packets {
                for decoded in decoder.decode(&packet.data).unwrap() {
                    assert_eq!((decoded.width(), decoded.height()), (width, height));
                    if index >= 5 {
                        let psnr = luma_psnr(&original, &decoded);
                        assert!(psnr > 30.0, "frame {index}: PSNR {psnr:.1} dB");
                    }
                    decoded_count += 1;
                }
            }
        }
        assert_eq!(decoded_count, 30);
    }

    #[test]
    fn first_frame_and_forced_frames_are_keyframes() {
        let mut encoder = Vp9Encoder::new(EncoderSettings::new(160, 120, 500)).unwrap();
        let keyframes: Vec<bool> = (0..6)
            .map(|index| {
                let packets = encoder
                    .encode(&frame(160, 120, index), i64::from(index) * 33, index == 4)
                    .unwrap();
                assert_eq!(packets.len(), 1);
                packets[0].keyframe
            })
            .collect();
        assert_eq!(keyframes, [true, false, false, false, true, false]);
    }

    #[test]
    fn decoder_can_join_stream_at_forced_keyframe() {
        let mut encoder = Vp9Encoder::new(EncoderSettings::new(160, 120, 500)).unwrap();
        for index in 0..3 {
            encoder
                .encode(&frame(160, 120, index), i64::from(index), false)
                .unwrap();
        }
        let keyframe = encoder.encode(&frame(160, 120, 3), 3, true).unwrap();
        let following = encoder.encode(&frame(160, 120, 4), 4, false).unwrap();
        let mut late_decoder = Vp9Decoder::new(1).unwrap();
        assert_eq!(late_decoder.decode(&keyframe[0].data).unwrap().len(), 1);
        assert_eq!(late_decoder.decode(&following[0].data).unwrap().len(), 1);
    }

    #[test]
    fn odd_dimensions_round_trip() {
        let mut encoder = Vp9Encoder::new(EncoderSettings::new(101, 57, 500)).unwrap();
        let mut decoder = Vp9Decoder::new(1).unwrap();
        let packets = encoder.encode(&frame(101, 57, 0), 0, false).unwrap();
        let decoded = decoder.decode(&packets[0].data).unwrap();
        assert_eq!((decoded[0].width(), decoded[0].height()), (101, 57));
        assert_eq!(decoded[0].u().len(), 51 * 29);
    }

    #[test]
    fn non_increasing_timestamps_are_made_monotonic() {
        let mut encoder = Vp9Encoder::new(EncoderSettings::new(160, 120, 500)).unwrap();
        let first = encoder.encode(&frame(160, 120, 0), 100, false).unwrap();
        let second = encoder.encode(&frame(160, 120, 1), 100, false).unwrap();
        let third = encoder.encode(&frame(160, 120, 2), 50, false).unwrap();
        assert_eq!(first[0].timestamp_ms, 100);
        assert_eq!(second[0].timestamp_ms, 101);
        assert_eq!(third[0].timestamp_ms, 102);
    }

    #[test]
    fn encoder_rejects_frame_of_wrong_size_and_accepts_bitrate_change() {
        let mut encoder = Vp9Encoder::new(EncoderSettings::new(160, 120, 500)).unwrap();
        assert!(matches!(
            encoder.encode(&frame(162, 120, 0), 0, false),
            Err(CodecError::FrameSizeMismatch { .. })
        ));
        assert!(encoder.set_bitrate(0).is_err());
        encoder.set_bitrate(1500).unwrap();
        assert_eq!(
            encoder.encode(&frame(160, 120, 0), 0, false).unwrap().len(),
            1
        );
    }

    #[test]
    fn decoder_rejects_empty_and_corrupt_input() {
        let mut decoder = Vp9Decoder::new(1).unwrap();
        assert_eq!(decoder.decode(&[]), Err(CodecError::InvalidPacket));
        assert!(
            decoder
                .decode(&[0xde, 0xad, 0xbe, 0xef, 0x00, 0x11])
                .is_err()
        );
        assert!(Vp9Decoder::new(0).is_err());
    }

    #[test]
    fn encoder_and_decoder_can_move_between_threads() {
        let mut encoder = Vp9Encoder::new(EncoderSettings::new(160, 120, 500)).unwrap();
        let packets = std::thread::spawn(move || encoder.encode(&frame(160, 120, 0), 0, false))
            .join()
            .unwrap()
            .unwrap();
        let mut decoder = Vp9Decoder::new(1).unwrap();
        let frames = std::thread::spawn(move || decoder.decode(&packets[0].data))
            .join()
            .unwrap()
            .unwrap();
        assert_eq!(frames.len(), 1);
    }
}
