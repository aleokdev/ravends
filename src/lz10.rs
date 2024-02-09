use std::io::Read;

use byteorder::ReadBytesExt;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Lz10DecompressionError {
    #[error("file read error")]
    Io(#[from] std::io::Error),
    #[error("invalid decompressed size (0 bytes)")]
    InvalidSize,
    #[error("error while referencing past data")]
    CannotReferencePastData,
    #[error("magic number does not match (found: 0x{found:x}, expected: 0x10)")]
    MagicNumberMismatch { found: u8 },
}

pub fn decompress_lz10(mut reader: impl Read) -> Result<Vec<u8>, Lz10DecompressionError> {
    let magic_num = reader.read_u8()?;
    if magic_num != 0x10 {
        return Err(Lz10DecompressionError::MagicNumberMismatch { found: magic_num });
    }
    let uncompressed_file_size = reader.read_u24::<byteorder::LittleEndian>()?;
    if uncompressed_file_size == 0 {
        return Err(Lz10DecompressionError::InvalidSize);
    }
    let mut output = Vec::with_capacity(uncompressed_file_size as usize);
    while let Ok(decision_byte) = reader.read_u8() {
        for bit in (0..8).rev().map(|idx| (decision_byte & (1 << idx)) != 0) {
            if bit {
                let pointer_data = reader.read_u16::<byteorder::BigEndian>()?;
                let length = (pointer_data >> 12) + 3;
                let offset = pointer_data & 0xFFF;
                if output.len() <= offset as usize {
                    return Err(Lz10DecompressionError::CannotReferencePastData);
                }
                let window_offset = output.len() - offset as usize - 1;
                for point_byte in 0..length as usize {
                    output.push(output[window_offset + point_byte]);
                }
            } else {
                output.push(reader.read_u8()?);
            }
            if output.len() >= uncompressed_file_size as usize {
                return Ok(output);
            }
        }
    }
    Ok(output)
}
