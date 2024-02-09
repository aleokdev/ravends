use std::{
    char::DecodeUtf16Error,
    io::{Read, Write},
    path::{Path, PathBuf},
    string::FromUtf16Error,
};

use anyhow::{anyhow, Context};
use clap::{Parser, Subcommand};
use std::fs;
use thiserror::Error;

use byteorder::ReadBytesExt;

#[derive(Error, Debug)]
enum Lz10DecompressionError {
    #[error("file read error")]
    Io(#[from] std::io::Error),
    #[error("invalid decompressed size (0)")]
    InvalidSize,
    #[error("error while referencing past data")]
    CannotReferencePastData,
    #[error("magic number does not match (found: 0x{found:x}, expected: 0x10)")]
    MagicNumberMismatch { found: u8 },
}

fn decompress_lz10(mut reader: impl Read) -> Result<Vec<u8>, Lz10DecompressionError> {
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

#[derive(Error, Debug)]
enum ParseTextError {
    #[error("file read error")]
    Io(#[from] std::io::Error),
    #[error("UTF-16 character decode error")]
    Utf16(#[from] DecodeUtf16Error),
}

fn parse_text_file(data: &[u8]) -> Result<Vec<String>, ParseTextError> {
    let mut header = data;
    let text_count = header.read_u32::<byteorder::LittleEndian>()?;
    (0..text_count)
        .map(|_| {
            let pointer = header.read_u32::<byteorder::LittleEndian>()? as usize;
            let pointer_data = &data[pointer..];
            char::decode_utf16(
                pointer_data
                    .chunks_exact(2)
                    .map(|ch| u16::from_le_bytes(ch.try_into().unwrap()))
                    .take_while(|&ch| ch != 0),
            )
            .collect::<Result<String, _>>()
            .map_err(ParseTextError::from)
        })
        .collect::<Result<Vec<String>, _>>()
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
    /// Try to decompress a file using the LZ10 algorithm
    Decompress {
        /// Path of the file to decompress
        path: PathBuf,
        /// Where to place the resulting file
        ///
        /// If empty, `path + .decomp` will be used instead
        target_path: Option<PathBuf>,
    },
    /// Try to identify a file from its contents
    Identify {
        /// Path of the file to identify
        path: PathBuf,
    },
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
        Commands::Decompress { path, target_path } => {
            let target_path = target_path.unwrap_or_else(|| path.join(".decomp"));

            let reader = std::io::BufReader::new(fs::File::open(path).context("failed to open file given")?);

            let data = decompress_lz10(reader).context("failed to decompress file")?;

            fs::File::create(target_path)?.write_all(&data)?;
        }
        Commands::Identify { path } => {
            let mut data = Vec::new();
            fs::File::open(path)
                .context("could not open file to idenfify")?
                .read_to_end(&mut data)
                .context("could not read file to idenfify")?;

            if let Ok(decompressed_data) = decompress_lz10(data.as_slice()) {
                print!("compressed LZ10 file, ");
                match parse_text_file(&decompressed_data) {
                    Ok(_) => {
                        println!("text file");
                    }
                    Err(_) => {
                        println!("unknown contents");
                    }
                };
            } else {
                println!("unknown format");
            };
        }

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

            let fs = nitro_fs::FileSystem::new(
                &rom_data[fnt_addr..(fnt_addr + fnt_size)],
                &rom_data[fat_addr..(fat_addr + fat_size)],
            )?;
            for entry in fs.files() {
                print!("{:?}: ", entry.path);

                let mut target_entry_path = target_path.join(&entry.path);
                std::fs::create_dir_all(target_entry_path.parent().unwrap())
                    .context("failed to create directory in target")?;

                let file_data = &rom_data[entry.alloc.start as usize..entry.alloc.end as usize];

                if let Ok(decompressed_data) = decompress_lz10(file_data) {
                    print!("compressed LZ10 file, ");
                    target_entry_path.set_extension("decomp");
                    let data_to_write = match parse_text_file(&decompressed_data) {
                        Ok(strings) => {
                            println!("text file");
                            target_entry_path.set_extension("txt");
                            strings
                                .into_iter()
                                .enumerate()
                                .map(|(idx, str)| {
                                    include_str!("text_entry_template")
                                        .replace("{{text}}", &str)
                                        .replace("{{index}}", &idx.to_string())
                                })
                                .collect::<String>()
                                .into_bytes()
                        }
                        Err(_) => {
                            println!("unknown contents");
                            decompressed_data
                        }
                    };

                    fs::File::create(target_entry_path)
                        .context("failed to create file in target directory")?
                        .write_all(&data_to_write)
                        .context("failed to write file in target directory")?;
                } else {
                    println!("unknown format");
                    fs::File::create(target_entry_path)
                        .context("failed to create file in target directory")?
                        .write_all(&file_data)
                        .context("failed to write file in target directory")?;
                };
            }
        }

        Commands::Pack { fs_path, rom_path } => {
            todo!()
        }
    }

    Ok(())
}
