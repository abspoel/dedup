use clap::Parser;
use generic_array::GenericArray;
use multimap::MultiMap;
use number_prefix::NumberPrefix;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::{fs, io};
use walkdir::{DirEntry, WalkDir};

const HASH_BLOCK_LEN: usize = 65536;
const HASH_BUFLEN: usize = 65536;

#[derive(Parser)]
#[command(
    name = "dedup",
    about = "Find duplicate files in a directory structure"
)]
struct Options {
    #[arg(
        short,
        long,
        default_value_t = 0,
        help = "Minimum size (in bytes) of files to search"
    )]
    min_size: u64,

    #[arg(
        short,
        long,
        help = "Print file names and sizes of the found duplicates"
    )]
    verbose: bool,

    #[arg(
        long,
        short = 'd',
        help = "Do not search files beyond this depth. Files in the specified paths are considered depth 1."
    )]
    max_depth: Option<usize>,

    #[arg(
        short = 's',
        long = "symlink",
        group = "mode",
        help = "Replace duplicate files by symlinks"
    )]
    replace_by_symlink: bool,

    #[arg(long, group = "mode", help = "Remove duplicate files")]
    remove: bool,

    #[arg(required = true, help = "Directories to search")]
    paths: Vec<PathBuf>,
}

type Hash = GenericArray<u8, sha2::digest::consts::U32>;

#[derive(Debug)]
enum SizeMapEntry {
    One(PathBuf),
    Multiple(MultiMap<Hash, PathBuf>),
}

struct Index {
    size_map: BTreeMap<u64, SizeMapEntry>,
    full_hashes: HashMap<PathBuf, Hash>,
}

fn short_hash(path: &Path) -> io::Result<Hash> {
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(path)?;
    let mut buf = [0u8; HASH_BLOCK_LEN];
    let mut total_read: usize = 0;

    while total_read < HASH_BLOCK_LEN {
        let read_bytes = file.read(&mut buf[total_read..])?;
        if read_bytes == 0 {
            break;
        }
        total_read += read_bytes;
    }

    hasher.update(buf);
    let mut hash = Hash::default();
    hasher.finalize_into(&mut hash);
    Ok(hash)
}

fn compute_full_hash(path: &Path) -> io::Result<Hash> {
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(path)?;
    let mut buf = [0u8; HASH_BUFLEN];

    loop {
        let read_bytes = file.read(&mut buf)?;
        if read_bytes == 0 {
            break;
        }
        hasher.update(buf);
    }

    let mut hash = Hash::default();
    hasher.finalize_into(&mut hash);
    Ok(hash)
}

fn full_hash(path: &Path, full_hashes: &mut HashMap<PathBuf, Hash>) -> io::Result<Hash> {
    use std::collections::hash_map::Entry;
    match full_hashes.entry(path.to_path_buf()) {
        Entry::Occupied(o) => Ok(*o.get()),
        Entry::Vacant(v) => {
            let hash = compute_full_hash(path)?;
            v.insert(hash);
            Ok(hash)
        }
    }
}

fn check_index(entry: &DirEntry, index: &mut Index) -> io::Result<Option<PathBuf>> {
    use std::collections::btree_map::Entry;
    let size = entry.metadata()?.len();
    let index_entry = index.size_map.entry(size);
    let path = entry.path();
    match index_entry {
        Entry::Occupied(mut o) => match o.get_mut() {
            SizeMapEntry::One(prev_path) => {
                let mut hash_map: MultiMap<Hash, PathBuf> = MultiMap::new();
                let prev_hash = short_hash(prev_path)?;
                hash_map.insert(prev_hash, prev_path.clone());

                let new_hash = short_hash(path)?;
                if new_hash == prev_hash
                    && full_hash(prev_path, &mut index.full_hashes)?
                        == full_hash(path, &mut index.full_hashes)?
                {
                    return Ok(Some(prev_path.clone()));
                }
                hash_map.insert(new_hash, path.to_path_buf());
                *o.get_mut() = SizeMapEntry::Multiple(hash_map);
            }
            SizeMapEntry::Multiple(hash_map) => {
                let new_hash = short_hash(path)?;
                if let Some(slice) = hash_map.get_slice(&new_hash) {
                    for prev_path in slice {
                        if full_hash(prev_path, &mut index.full_hashes)?
                            == full_hash(path, &mut index.full_hashes)?
                        {
                            return Ok(Some(prev_path.clone()));
                        }
                    }
                }
                hash_map.insert(new_hash, path.to_path_buf());
            }
        },
        Entry::Vacant(v) => {
            v.insert(SizeMapEntry::One(path.to_path_buf()));
        }
    };

    Ok(None)
}

fn relative_path(base: &Path, target: &Path) -> io::Result<PathBuf> {
    // Should not be called where path or target is symlink
    let abs_base = base.canonicalize()?;
    let abs_target = target.canonicalize()?;

    let mut iter_base = abs_base.components();
    let mut iter_target = abs_target.components().peekable();

    loop {
        let c_base = iter_base.next();
        let c_target = iter_target.peek();
        if c_base.is_none() || c_target.is_none() || c_base.unwrap() != *c_target.unwrap() {
            break;
        }
        iter_target.next();
    }

    let relative = iter_base
        .map(|a| match a {
            std::path::Component::Normal(_) => std::path::Component::ParentDir,
            _ => panic!(),
        })
        .chain(iter_target)
        .collect::<PathBuf>();
    Ok(relative)
}

fn format_bytes(num: u64) -> String {
    match NumberPrefix::binary(num as f64) {
        NumberPrefix::Standalone(bytes) => {
            format!("{} bytes", bytes)
        }
        NumberPrefix::Prefixed(prefix, n) => {
            format!("{:.1} {}B", n, prefix)
        }
    }
}

fn main() -> anyhow::Result<()> {
    let options = Options::parse();

    let mut index = Index {
        size_map: BTreeMap::new(),
        full_hashes: HashMap::new(),
    };

    let mut num_files = 0;
    let mut num_actions = 0;
    let mut saved_bytes = 0;

    for dir in options.paths {
        let mut walk = WalkDir::new(dir);
        if let Some(max_depth) = options.max_depth {
            walk = walk.max_depth(max_depth);
        }
        for _entry in walk {
            let entry = &_entry?;
            let size = entry.metadata()?.len();
            if entry.file_type().is_file() && size > options.min_size {
                if let Some(prev_path) = check_index(entry, &mut index)? {
                    if prev_path != entry.path() {
                        let rel = relative_path(entry.path(), &prev_path)?;
                        if options.remove || options.replace_by_symlink {
                            fs::remove_file(entry.path())?;
                            if options.replace_by_symlink {
                                std::os::unix::fs::symlink(&rel, entry.path())?;
                            }
                        }
                        if options.verbose {
                            if options.remove {
                                println!("({}) remove {:?}", format_bytes(size), entry.path());
                            } else {
                                println!(
                                    "({}) link {:?} -> {:?}",
                                    format_bytes(size),
                                    entry.path(),
                                    rel
                                );
                            }
                        }
                        saved_bytes += size;
                        num_actions += 1;
                    }
                }
                num_files += 1;
            }
        }
    }

    print!("Processed {} files. ", num_files);
    if options.remove || options.replace_by_symlink {
        if options.remove {
            print!("Removed {} files", num_actions);
        } else {
            /* if options.replace_by_symlink  */
            print!("Created {} symlinks", num_actions);
        }
        println!(", saving {}.", format_bytes(saved_bytes));
    } else {
        println!(
            "Found {} duplicates. Removing them would save {}.",
            num_actions,
            format_bytes(saved_bytes)
        );
    }
    anyhow::Ok(())
}
