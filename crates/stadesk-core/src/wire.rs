use crate::error::CoreError;

pub struct Writer {
    buffer: Vec<u8>,
}

impl Writer {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(capacity),
        }
    }

    pub fn u8(&mut self, value: u8) -> &mut Self {
        self.buffer.push(value);
        self
    }

    pub fn u16(&mut self, value: u16) -> &mut Self {
        self.buffer.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn u32(&mut self, value: u32) -> &mut Self {
        self.buffer.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn bytes(&mut self, value: &[u8]) -> &mut Self {
        self.buffer.extend_from_slice(value);
        self
    }

    pub fn finish(self) -> Vec<u8> {
        self.buffer
    }
}

pub struct Reader<'a> {
    remaining: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self { remaining: input }
    }

    pub fn u8(&mut self) -> Result<u8, CoreError> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u16(&mut self) -> Result<u16, CoreError> {
        Ok(u16::from_be_bytes(self.array::<2>()?))
    }

    pub fn u32(&mut self) -> Result<u32, CoreError> {
        Ok(u32::from_be_bytes(self.array::<4>()?))
    }

    pub fn slice(&mut self, length: usize) -> Result<&'a [u8], CoreError> {
        if self.remaining.len() < length {
            return Err(CoreError::Malformed);
        }
        let (head, tail) = self.remaining.split_at(length);
        self.remaining = tail;
        Ok(head)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], CoreError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.slice(N)?);
        Ok(out)
    }

    pub fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.remaining)
    }

    pub fn finish(self) -> Result<(), CoreError> {
        if self.remaining.is_empty() {
            Ok(())
        } else {
            Err(CoreError::Malformed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_returns_written_fields_in_order() {
        let mut writer = Writer::with_capacity(8);
        writer.u8(5).u16(0x0102).u32(0x0a0b0c0d).bytes(&[9, 8, 7]);
        let encoded = writer.finish();
        let mut reader = Reader::new(&encoded);
        assert_eq!(reader.u8().unwrap(), 5);
        assert_eq!(reader.u16().unwrap(), 0x0102);
        assert_eq!(reader.u32().unwrap(), 0x0a0b0c0d);
        assert_eq!(reader.array::<3>().unwrap(), [9, 8, 7]);
        assert!(reader.finish().is_ok());
    }

    #[test]
    fn reader_rejects_truncated_and_trailing_input() {
        assert_eq!(Reader::new(&[1]).u16(), Err(CoreError::Malformed));
        let mut reader = Reader::new(&[1, 2, 3]);
        reader.u16().unwrap();
        assert_eq!(reader.finish(), Err(CoreError::Malformed));
    }
}
