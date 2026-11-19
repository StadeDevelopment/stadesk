use crate::frame::{BGRA_BYTES, CapturedFrame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rotation {
    Identity,
    Clockwise90,
    Clockwise180,
    Clockwise270,
}

impl Rotation {
    pub(crate) fn output_size(self, width: u32, height: u32) -> (u32, u32) {
        match self {
            Self::Identity | Self::Clockwise180 => (width, height),
            Self::Clockwise90 | Self::Clockwise270 => (height, width),
        }
    }
}

pub(crate) fn rotate_into_frame(
    source: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    rotation: Rotation,
) -> Option<CapturedFrame> {
    let (source_width, source_height) = (width as usize, height as usize);
    if width == 0
        || height == 0
        || stride < source_width * BGRA_BYTES
        || source.len() < stride * (source_height - 1) + source_width * BGRA_BYTES
    {
        return None;
    }
    let (output_width, output_height) = rotation.output_size(width, height);
    let (columns, rows) = (output_width as usize, output_height as usize);
    let mut bgra = vec![0u8; columns * rows * BGRA_BYTES];
    if rotation == Rotation::Identity {
        for row in 0..rows {
            let line = &source[row * stride..row * stride + columns * BGRA_BYTES];
            bgra[row * columns * BGRA_BYTES..(row + 1) * columns * BGRA_BYTES]
                .copy_from_slice(line);
        }
    } else {
        for y in 0..rows {
            for x in 0..columns {
                let (source_x, source_y) = match rotation {
                    Rotation::Identity => (x, y),
                    Rotation::Clockwise90 => (y, source_height - 1 - x),
                    Rotation::Clockwise180 => (source_width - 1 - x, source_height - 1 - y),
                    Rotation::Clockwise270 => (source_width - 1 - y, x),
                };
                let from = source_y * stride + source_x * BGRA_BYTES;
                let to = (y * columns + x) * BGRA_BYTES;
                bgra[to..to + BGRA_BYTES].copy_from_slice(&source[from..from + BGRA_BYTES]);
            }
        }
    }
    Some(CapturedFrame {
        width: output_width,
        height: output_height,
        bgra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(width: u32, height: u32) -> Vec<u8> {
        (0..width * height)
            .flat_map(|index| [index as u8, 0, 0, 255])
            .collect()
    }

    fn ids(frame: &CapturedFrame) -> Vec<u8> {
        frame
            .bgra
            .chunks(BGRA_BYTES)
            .map(|pixel| pixel[0])
            .collect()
    }

    #[test]
    fn identity_copy_drops_row_padding() {
        let mut padded = vec![9u8; 2 * 12];
        padded[..8].copy_from_slice(&[1, 0, 0, 255, 2, 0, 0, 255]);
        padded[12..20].copy_from_slice(&[3, 0, 0, 255, 4, 0, 0, 255]);
        let frame = rotate_into_frame(&padded, 2, 2, 12, Rotation::Identity).unwrap();
        assert_eq!(ids(&frame), [1, 2, 3, 4]);
    }

    #[test]
    fn rotations_place_pixels_like_a_turned_monitor() {
        let source = numbered(3, 2);
        let clockwise_90 = rotate_into_frame(&source, 3, 2, 12, Rotation::Clockwise90).unwrap();
        assert_eq!((clockwise_90.width, clockwise_90.height), (2, 3));
        assert_eq!(ids(&clockwise_90), [3, 0, 4, 1, 5, 2]);
        let half_turn = rotate_into_frame(&source, 3, 2, 12, Rotation::Clockwise180).unwrap();
        assert_eq!(ids(&half_turn), [5, 4, 3, 2, 1, 0]);
        let clockwise_270 = rotate_into_frame(&source, 3, 2, 12, Rotation::Clockwise270).unwrap();
        assert_eq!((clockwise_270.width, clockwise_270.height), (2, 3));
        assert_eq!(ids(&clockwise_270), [2, 5, 1, 4, 0, 3]);
    }

    #[test]
    fn four_quarter_turns_restore_the_original() {
        let source = numbered(4, 3);
        let mut frame = rotate_into_frame(&source, 4, 3, 16, Rotation::Identity).unwrap();
        for _ in 0..4 {
            let stride = frame.width as usize * BGRA_BYTES;
            frame = rotate_into_frame(
                &frame.bgra,
                frame.width,
                frame.height,
                stride,
                Rotation::Clockwise90,
            )
            .unwrap();
        }
        assert_eq!(frame.bgra, source);
    }

    #[test]
    fn rotation_rejects_short_buffers() {
        assert!(rotate_into_frame(&[0u8; 15], 2, 2, 8, Rotation::Identity).is_none());
        assert!(rotate_into_frame(&[0u8; 16], 2, 2, 7, Rotation::Identity).is_none());
        assert!(rotate_into_frame(&[], 0, 2, 0, Rotation::Identity).is_none());
    }
}
