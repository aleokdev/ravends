use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context};
use clap::{Parser, Subcommand};
use thiserror::Error;
use std::fs;

use byteorder::ReadBytesExt;

#[derive(Error, Debug)]
enum Lz10DecompressionError {
    #[error("file read error")]
    Io(#[from] std::io::Error),
    #[error("invalid decompressed size (0)")]
    InvalidSize,
    #[error("error while referencing past data; data might not be LZ10 compressed")]
    CannotReferencePastData,
    #[error("magic number does not match")]
    MagicNumberMismatch
}

fn decompress_lz10(mut reader: impl Read) -> Result<Vec<u8>, Lz10DecompressionError> {
    let magic_num = reader.read_u8()?;
    if magic_num != 0x10 {
        return Err(Lz10DecompressionError::MagicNumberMismatch);
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

#[derive(Debug, Parser)]
#[command(name = "ravends")]
#[command(about = "NDS unpacking & patching tool", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Unpack a ROM file's contents to a directory
    Unpack {
        /// The ROM file to unpack
        rom_path: PathBuf,
        /// Where to unpack the resulting files, without creating a parent folder for them
        ///
        /// If empty, the software will create a folder of the same name as the ROM in its same path, and unpack it there.
        target_path: Option<PathBuf>,
    },
    /// Pack a directory's contents to a ROM file
    Pack {
        /// The directory to pack into a ROM
        fs_path: PathBuf,
        /// Where to place the resulting ROM
        ///
        /// If empty, the software will place the ROM alongside the directory given, with a '.nds' extension at the end.
        rom_path: Option<PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    let args = Cli::parse();

    match args.command {
        Commands::Unpack {
            rom_path,
            target_path,
        } => {
            let target_path = target_path.unwrap_or_else(|| rom_path.with_extension(""));
            std::fs::create_dir_all(&target_path).context("failed to create target directory")?;

            let mut rom_data = Vec::new();
            std::io::BufReader::new(fs::File::open(rom_path)?).read_to_end(&mut rom_data)?;

            let fnt_addr = u32::from_le_bytes(rom_data[0x40..=0x43].try_into().unwrap()) as usize;
            let fnt_size = u32::from_le_bytes(rom_data[0x44..=0x47].try_into().unwrap()) as usize;

            let fat_addr = u32::from_le_bytes(rom_data[0x48..=0x4B].try_into().unwrap()) as usize;
            let fat_size = u32::from_le_bytes(rom_data[0x4C..=0x4F].try_into().unwrap()) as usize;

            let fs = nitro_fs::FileSystem::new(&rom_data[fnt_addr..(fnt_addr+fnt_size)], &rom_data[fat_addr..(fat_addr+fat_size)])?;
            for entry in fs.files() {
                print!("{:?}: ", entry.path);
                
                let target_entry_path = target_path.join(&entry.path);
                std::fs::create_dir_all(target_entry_path.parent().unwrap()).context("failed to create directory in target")?;

                let file_data = &rom_data[entry.alloc.start as usize .. entry.alloc.end as usize];
                
                if let Ok(decompressed_data) = decompress_lz10(std::io::BufReader::new(file_data)) {
                    println!("compressed LZ10 file, unknown contents");
                    fs::File::create(target_entry_path).context("failed to create file in target directory")?.write_all(&decompressed_data).context("failed to write file in target directory")?;
                } else {
                    println!("unknown format");
                    fs::File::create(target_entry_path).context("failed to create file in target directory")?.write_all(&file_data).context("failed to write file in target directory")?;
                };
            }
        }

        Commands::Pack { fs_path, rom_path } => {
            todo!()
        }
    }

    Ok(())
}
