use crate::frame::{BGRA_BYTES, CapturedFrame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PointerKind {
    #[cfg(any(windows, test))]
    Monochrome,
    #[cfg(any(windows, test))]
    Color,
    #[cfg(any(target_os = "linux", test))]
    PremultipliedColor,
    #[cfg(any(windows, test))]
    MaskedColor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PointerShape {
    pub(crate) kind: PointerKind,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pitch: usize,
    pub(crate) data: Vec<u8>,
}

impl PointerShape {
    fn visible_height(&self) -> u32 {
        match self.kind {
            #[cfg(any(windows, test))]
            PointerKind::Monochrome => self.height / 2,
            _ => self.height,
        }
    }

    fn required_len(&self) -> usize {
        if self.height == 0 {
            return 0;
        }
        let last_row_bytes = match self.kind {
            #[cfg(any(windows, test))]
            PointerKind::Monochrome => (self.width as usize).div_ceil(8),
            _ => self.width as usize * BGRA_BYTES,
        };
        self.pitch * (self.height as usize - 1) + last_row_bytes
    }

    #[cfg(any(windows, test))]
    fn mask_bit(&self, row: usize, column: usize) -> bool {
        self.data[row * self.pitch + column / 8] & (0x80 >> (column % 8)) != 0
    }
}

#[cfg(any(windows, test))]
fn blend(source: u8, destination: u8, alpha: u8) -> u8 {
    let alpha = u16::from(alpha);
    ((u16::from(source) * alpha + u16::from(destination) * (255 - alpha) + 127) / 255) as u8
}

#[cfg(any(target_os = "linux", test))]
fn blend_premultiplied(source: u8, destination: u8, alpha: u8) -> u8 {
    let remaining = (u16::from(destination) * (255 - u16::from(alpha)) + 127) / 255;
    (u16::from(source) + remaining).min(255) as u8
}

pub(crate) fn draw_pointer(frame: &mut CapturedFrame, shape: &PointerShape, left: i32, top: i32) {
    if shape.data.len() < shape.required_len() {
        return;
    }
    let frame_width = frame.width as i64;
    let frame_height = frame.height as i64;
    for row in 0..shape.visible_height() as usize {
        let y = i64::from(top) + row as i64;
        if y < 0 || y >= frame_height {
            continue;
        }
        for column in 0..shape.width as usize {
            let x = i64::from(left) + column as i64;
            if x < 0 || x >= frame_width {
                continue;
            }
            let offset = (y as usize * frame.width as usize + x as usize) * BGRA_BYTES;
            let pixel = &mut frame.bgra[offset..offset + BGRA_BYTES];
            match shape.kind {
                #[cfg(any(windows, test))]
                PointerKind::Monochrome => {
                    let and = shape.mask_bit(row, column);
                    let xor = shape.mask_bit(row + shape.visible_height() as usize, column);
                    for channel in &mut pixel[..3] {
                        let kept = if and { *channel } else { 0 };
                        *channel = if xor { kept ^ 0xff } else { kept };
                    }
                }
                #[cfg(any(windows, test))]
                PointerKind::Color => {
                    let at = row * shape.pitch + column * BGRA_BYTES;
                    let source = &shape.data[at..at + BGRA_BYTES];
                    for channel in 0..3 {
                        pixel[channel] = blend(source[channel], pixel[channel], source[3]);
                    }
                }
                #[cfg(any(target_os = "linux", test))]
                PointerKind::PremultipliedColor => {
                    let at = row * shape.pitch + column * BGRA_BYTES;
                    let source = &shape.data[at..at + BGRA_BYTES];
                    for channel in 0..3 {
                        pixel[channel] =
                            blend_premultiplied(source[channel], pixel[channel], source[3]);
                    }
                }
                #[cfg(any(windows, test))]
                PointerKind::MaskedColor => {
                    let at = row * shape.pitch + column * BGRA_BYTES;
                    let source = &shape.data[at..at + BGRA_BYTES];
                    for channel in 0..3 {
                        pixel[channel] = if source[3] == 0 {
                            source[channel]
                        } else {
                            pixel[channel] ^ source[channel]
                        };
                    }
                }
            }
            pixel[3] = 0xff;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray_frame(width: u32, height: u32, value: u8) -> CapturedFrame {
        CapturedFrame {
            width,
            height,
            bgra: [value, value, value, 255].repeat((width * height) as usize),
        }
    }

    fn pixel(frame: &CapturedFrame, x: usize, y: usize) -> [u8; 4] {
        let offset = (y * frame.width as usize + x) * BGRA_BYTES;
        frame.bgra[offset..offset + 4].try_into().unwrap()
    }

    #[test]
    fn monochrome_pointer_applies_and_then_xor_masks() {
        let shape = PointerShape {
            kind: PointerKind::Monochrome,
            width: 4,
            height: 2,
            pitch: 1,
            data: vec![0b1010_0000, 0b1100_0000],
        };
        let mut frame = gray_frame(4, 1, 0x40);
        draw_pointer(&mut frame, &shape, 0, 0);
        assert_eq!(pixel(&frame, 0, 0), [0xbf, 0xbf, 0xbf, 255]);
        assert_eq!(pixel(&frame, 1, 0), [0xff, 0xff, 0xff, 255]);
        assert_eq!(pixel(&frame, 2, 0), [0x40, 0x40, 0x40, 255]);
        assert_eq!(pixel(&frame, 3, 0), [0x00, 0x00, 0x00, 255]);
    }

    #[test]
    fn color_pointer_alpha_blends() {
        let shape = PointerShape {
            kind: PointerKind::Color,
            width: 3,
            height: 1,
            pitch: 12,
            data: vec![255, 255, 255, 255, 255, 255, 255, 0, 255, 255, 255, 128],
        };
        let mut frame = gray_frame(3, 1, 0);
        draw_pointer(&mut frame, &shape, 0, 0);
        assert_eq!(pixel(&frame, 0, 0), [255, 255, 255, 255]);
        assert_eq!(pixel(&frame, 1, 0), [0, 0, 0, 255]);
        assert_eq!(pixel(&frame, 2, 0), [128, 128, 128, 255]);
    }

    #[test]
    fn premultiplied_pointer_adds_source_over_attenuated_destination() {
        let shape = PointerShape {
            kind: PointerKind::PremultipliedColor,
            width: 3,
            height: 1,
            pitch: 12,
            data: vec![200, 100, 50, 255, 0, 0, 0, 0, 64, 64, 64, 128],
        };
        let mut frame = gray_frame(3, 1, 200);
        draw_pointer(&mut frame, &shape, 0, 0);
        assert_eq!(pixel(&frame, 0, 0), [200, 100, 50, 255]);
        assert_eq!(pixel(&frame, 1, 0), [200, 200, 200, 255]);
        assert_eq!(pixel(&frame, 2, 0), [164, 164, 164, 255]);
    }

    #[test]
    fn masked_color_pointer_replaces_or_xors() {
        let shape = PointerShape {
            kind: PointerKind::MaskedColor,
            width: 2,
            height: 1,
            pitch: 8,
            data: vec![0x10, 0x20, 0x30, 0x00, 0xff, 0x0f, 0xf0, 0xff],
        };
        let mut frame = gray_frame(2, 1, 0x55);
        draw_pointer(&mut frame, &shape, 0, 0);
        assert_eq!(pixel(&frame, 0, 0), [0x10, 0x20, 0x30, 255]);
        assert_eq!(pixel(&frame, 1, 0), [0xaa, 0x5a, 0xa5, 255]);
    }

    #[test]
    fn pointer_is_clipped_at_every_edge() {
        let shape = PointerShape {
            kind: PointerKind::Color,
            width: 3,
            height: 3,
            pitch: 12,
            data: [0, 0, 255, 255].repeat(9),
        };
        for (left, top) in [(-1, -1), (1, 1), (-5, 0), (0, 10)] {
            let mut frame = gray_frame(3, 3, 0);
            draw_pointer(&mut frame, &shape, left, top);
            let red = frame.bgra.chunks(4).filter(|p| p[2] == 255).count();
            let expected = match (left, top) {
                (-1, -1) => 4,
                (1, 1) => 4,
                _ => 0,
            };
            assert_eq!(red, expected, "at {left},{top}");
        }
    }

    #[test]
    fn truncated_pointer_data_is_ignored() {
        let shape = PointerShape {
            kind: PointerKind::Color,
            width: 2,
            height: 2,
            pitch: 8,
            data: vec![255; 15],
        };
        let mut frame = gray_frame(2, 2, 7);
        draw_pointer(&mut frame, &shape, 0, 0);
        assert!(frame.bgra.chunks(4).all(|p| p[..3] == [7, 7, 7]));
    }
}
