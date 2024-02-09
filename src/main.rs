use std::{
    char::DecodeUtf16Error,
    io::{Read, Write},
    path::{Path, PathBuf},
    string::FromUtf16Error,
};

use anyhow::{anyhow, Context};
use clap::{Parser, Subcommand};
use lz10::decompress_lz10;
use std::fs;
use thiserror::Error;

use byteorder::ReadBytesExt;

mod lz10;

#[derive(Error, Debug)]
enum ParseTextError {
    #[error("file read error")]
    Io(#[from] std::io::Error),
    #[error("UTF-16 character decode error")]
    Utf16(#[from] DecodeUtf16Error),
    #[error("invalid pointer found on header")]
    InvalidPointer,
}

fn parse_text_file(data: &[u8]) -> Result<Vec<String>, ParseTextError> {
    let mut header = data;
    let text_count = header.read_u32::<byteorder::LittleEndian>()? as usize;
    let header_size = text_count * std::mem::size_of::<u32>();
    (0..text_count)
        .map(|_| {
            let pointer = header.read_u32::<byteorder::LittleEndian>()? as usize;
            if pointer < header_size {
                return Err(ParseTextError::InvalidPointer);
            }
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
        .collect()
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

        /// If set, the software will not do any modifications on the file system
        #[arg(long, default_value_t = false)]
        dry_run: bool,
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

            let reader =
                std::io::BufReader::new(fs::File::open(path).context("failed to open file given")?);

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
            dry_run,
        } => {
            let target_path = target_path.unwrap_or_else(|| rom_path.with_extension(""));
            if !dry_run {
                std::fs::create_dir_all(&target_path)
                    .context("failed to create target directory")?;
            }

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
                if !dry_run {
                    std::fs::create_dir_all(target_entry_path.parent().unwrap())
                        .context("failed to create directory in target")?;
                }

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

                    if !dry_run {
                        fs::File::create(target_entry_path)
                            .context("failed to create file in target directory")?
                            .write_all(&data_to_write)
                            .context("failed to write file in target directory")?;
                    }
                } else {
                    println!("unknown format");

                    if !dry_run {
                        fs::File::create(target_entry_path)
                            .context("failed to create file in target directory")?
                            .write_all(&file_data)
                            .context("failed to write file in target directory")?;
                    }
                };
            }
        }

        Commands::Pack { fs_path, rom_path } => {
            todo!()
        }
    }

    Ok(())
}
