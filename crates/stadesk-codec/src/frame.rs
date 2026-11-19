use crate::error::CodecError;

pub const MAX_DIMENSION: u32 = 16_384;
const BLACK_LUMA: u8 = 16;
const NEUTRAL_CHROMA: u8 = 128;
const BGRA_BYTES: usize = 4;

#[derive(Clone, PartialEq, Eq)]
pub struct I420Frame {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

fn validate(width: u32, height: u32) -> Result<(), CodecError> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(CodecError::InvalidDimensions { width, height });
    }
    Ok(())
}

fn luma_from_rgb(red: i32, green: i32, blue: i32) -> u8 {
    (((66 * red + 129 * green + 25 * blue + 128) >> 8) + 16) as u8
}

fn chroma_from_rgb(red: i32, green: i32, blue: i32) -> (u8, u8) {
    let u = ((-38 * red - 74 * green + 112 * blue + 128) >> 8) + 128;
    let v = ((112 * red - 94 * green - 18 * blue + 128) >> 8) + 128;
    (u as u8, v as u8)
}

fn clamp_channel(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

impl I420Frame {
    pub fn black(width: u32, height: u32) -> Result<Self, CodecError> {
        validate(width, height)?;
        let luma_len = width as usize * height as usize;
        let chroma_len = Self::chroma_len(width, height);
        let mut data = vec![BLACK_LUMA; luma_len + 2 * chroma_len];
        data[luma_len..].fill(NEUTRAL_CHROMA);
        Ok(Self {
            width,
            height,
            data,
        })
    }

    pub(crate) fn chroma_width_of(width: u32) -> usize {
        width.div_ceil(2) as usize
    }

    pub(crate) fn chroma_height_of(height: u32) -> usize {
        height.div_ceil(2) as usize
    }

    fn chroma_len(width: u32, height: u32) -> usize {
        Self::chroma_width_of(width) * Self::chroma_height_of(height)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn chroma_width(&self) -> usize {
        Self::chroma_width_of(self.width)
    }

    pub fn chroma_height(&self) -> usize {
        Self::chroma_height_of(self.height)
    }

    fn luma_len(&self) -> usize {
        self.width as usize * self.height as usize
    }

    pub fn y(&self) -> &[u8] {
        &self.data[..self.luma_len()]
    }

    pub fn u(&self) -> &[u8] {
        let start = self.luma_len();
        &self.data[start..start + Self::chroma_len(self.width, self.height)]
    }

    pub fn v(&self) -> &[u8] {
        let start = self.luma_len() + Self::chroma_len(self.width, self.height);
        &self.data[start..]
    }

    pub(crate) fn planes_mut(&mut self) -> (&mut [u8], &mut [u8], &mut [u8]) {
        let luma_len = self.luma_len();
        let chroma_len = Self::chroma_len(self.width, self.height);
        let (y, rest) = self.data.split_at_mut(luma_len);
        let (u, v) = rest.split_at_mut(chroma_len);
        (y, u, v)
    }

    pub(crate) fn as_ptr(&self) -> *const u8 {
        self.data.as_ptr()
    }

    pub fn from_bgra(
        width: u32,
        height: u32,
        bgra: &[u8],
        stride: usize,
    ) -> Result<Self, CodecError> {
        validate(width, height)?;
        let row_bytes = width as usize * BGRA_BYTES;
        let rows = height as usize;
        if stride < row_bytes || bgra.len() < stride * (rows - 1) + row_bytes {
            return Err(CodecError::BufferTooSmall);
        }
        let mut frame = Self::black(width, height)?;
        let columns = width as usize;
        let chroma_width = frame.chroma_width();
        let chroma_height = frame.chroma_height();
        let (y_plane, u_plane, v_plane) = frame.planes_mut();
        let pixel = |x: usize, y: usize| -> (i32, i32, i32) {
            let offset = y * stride + x * BGRA_BYTES;
            (
                i32::from(bgra[offset + 2]),
                i32::from(bgra[offset + 1]),
                i32::from(bgra[offset]),
            )
        };
        for y in 0..rows {
            for x in 0..columns {
                let (red, green, blue) = pixel(x, y);
                y_plane[y * columns + x] = luma_from_rgb(red, green, blue);
            }
        }
        for chroma_y in 0..chroma_height {
            for chroma_x in 0..chroma_width {
                let (mut red, mut green, mut blue, mut count) = (0, 0, 0, 0);
                for y in (2 * chroma_y)..(2 * chroma_y + 2).min(rows) {
                    for x in (2 * chroma_x)..(2 * chroma_x + 2).min(columns) {
                        let (r, g, b) = pixel(x, y);
                        red += r;
                        green += g;
                        blue += b;
                        count += 1;
                    }
                }
                let half = count / 2;
                let (u, v) = chroma_from_rgb(
                    (red + half) / count,
                    (green + half) / count,
                    (blue + half) / count,
                );
                u_plane[chroma_y * chroma_width + chroma_x] = u;
                v_plane[chroma_y * chroma_width + chroma_x] = v;
            }
        }
        Ok(frame)
    }

    pub fn to_bgra(&self) -> Vec<u8> {
        let columns = self.width as usize;
        let rows = self.height as usize;
        let chroma_width = self.chroma_width();
        let (y_plane, u_plane, v_plane) = (self.y(), self.u(), self.v());
        let mut out = vec![0u8; columns * rows * BGRA_BYTES];
        for y in 0..rows {
            for x in 0..columns {
                let c = i32::from(y_plane[y * columns + x]) - 16;
                let chroma_index = (y / 2) * chroma_width + x / 2;
                let d = i32::from(u_plane[chroma_index]) - 128;
                let e = i32::from(v_plane[chroma_index]) - 128;
                let offset = (y * columns + x) * BGRA_BYTES;
                out[offset] = clamp_channel((298 * c + 516 * d + 128) >> 8);
                out[offset + 1] = clamp_channel((298 * c - 100 * d - 208 * e + 128) >> 8);
                out[offset + 2] = clamp_channel((298 * c + 409 * e + 128) >> 8);
                out[offset + 3] = 255;
            }
        }
        out
    }
}

impl std::fmt::Debug for I420Frame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("I420Frame")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn gradient_bgra(width: u32, height: u32, shift: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                out.push((((x + shift) % width) * 255 / width) as u8);
                out.push((y * 255 / height) as u8);
                out.push(((x + y) * 255 / (width + height)) as u8);
                out.push(255);
            }
        }
        out
    }

    #[test]
    fn black_frame_has_expected_plane_sizes_for_odd_dimensions() {
        let frame = I420Frame::black(5, 3).unwrap();
        assert_eq!(frame.y().len(), 15);
        assert_eq!(frame.chroma_width(), 3);
        assert_eq!(frame.chroma_height(), 2);
        assert_eq!(frame.u().len(), 6);
        assert_eq!(frame.v().len(), 6);
        assert!(frame.y().iter().all(|&value| value == 16));
        assert!(frame.u().iter().chain(frame.v()).all(|&value| value == 128));
    }

    #[test]
    fn primary_colors_convert_to_studio_range_reference_values() {
        let cases: [([u8; 4], (u8, u8, u8)); 5] = [
            ([0, 0, 0, 255], (16, 128, 128)),
            ([255, 255, 255, 255], (235, 128, 128)),
            ([0, 0, 255, 255], (82, 90, 240)),
            ([0, 255, 0, 255], (144, 54, 34)),
            ([255, 0, 0, 255], (41, 240, 110)),
        ];
        for (bgra, (y, u, v)) in cases {
            let frame = I420Frame::from_bgra(1, 1, &bgra, 4).unwrap();
            assert_eq!((frame.y()[0], frame.u()[0], frame.v()[0]), (y, u, v));
        }
    }

    #[test]
    fn bgra_round_trip_of_flat_colors_is_close() {
        for color in [[0u8, 0, 0], [255, 255, 255], [30, 120, 200], [200, 10, 90]] {
            let bgra: Vec<u8> = std::iter::repeat_n([color[0], color[1], color[2], 255], 16)
                .flatten()
                .collect();
            let restored = I420Frame::from_bgra(4, 4, &bgra, 16).unwrap().to_bgra();
            for (original, back) in bgra.iter().zip(&restored) {
                assert!(
                    original.abs_diff(*back) <= 3,
                    "{color:?}: {original} vs {back}"
                );
            }
        }
    }

    #[test]
    fn from_bgra_honours_row_stride_padding() {
        let mut padded = vec![0u8; 2 * 16];
        padded[..8].copy_from_slice(&[255, 255, 255, 255, 255, 255, 255, 255]);
        padded[16..24].copy_from_slice(&[255, 255, 255, 255, 255, 255, 255, 255]);
        let frame = I420Frame::from_bgra(2, 2, &padded, 16).unwrap();
        assert!(frame.y().iter().all(|&value| value == 235));
    }

    #[test]
    fn from_bgra_rejects_invalid_input() {
        assert_eq!(
            I420Frame::from_bgra(0, 4, &[], 0),
            Err(CodecError::InvalidDimensions {
                width: 0,
                height: 4
            })
        );
        assert_eq!(
            I420Frame::black(MAX_DIMENSION + 1, 1),
            Err(CodecError::InvalidDimensions {
                width: MAX_DIMENSION + 1,
                height: 1
            })
        );
        assert_eq!(
            I420Frame::from_bgra(2, 2, &[0u8; 15], 8),
            Err(CodecError::BufferTooSmall)
        );
        assert_eq!(
            I420Frame::from_bgra(2, 2, &[0u8; 16], 7),
            Err(CodecError::BufferTooSmall)
        );
    }

    #[test]
    fn gradient_survives_conversion_round_trip() {
        let bgra = gradient_bgra(16, 16, 0);
        let restored = I420Frame::from_bgra(16, 16, &bgra, 64).unwrap().to_bgra();
        let worst = bgra
            .iter()
            .zip(&restored)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(worst <= 40, "worst channel error {worst}");
    }
}
