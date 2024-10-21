use clap::{command, Parser};
use once_cell::sync::Lazy;
use toml::Table;
use std::fs::{self, DirEntry};
use sysinfo::{MemoryRefreshKind, RefreshKind, System};
use regex::{Regex, RegexBuilder};
use memmap2::Mmap;

#[derive(PartialEq)]
enum SortingMethod { FL, SL, LS }
const KIB: usize = 1024;
const MIB: usize = 1048576;
const GIB: usize = 1073741824;

/// A simple program to lock files in memory.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Custom configuration path
    #[arg(short, long, required = false)]
    config: Option<String>,
    /// Outputs memory usage for configuration; this will load files into memory to get accurate usage
    #[arg(short, long, required = false)]
    usage: bool,
}

struct Lock {
    current_size: usize,
    max_file_size: usize,
    max_total_size: usize,
    memory_size: usize,
    sorting_method: SortingMethod
}

struct FileInfo {
    size: u64
}

static mut LOADED: Lazy<Vec<(String,Mmap)>> = Lazy::new(|| {
    Vec::new()
});

fn size_to_bytes(size: &str, lock: &Lock) -> Option<usize> {
    static STB_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d*)([m,k,g,%])?").unwrap());
    if let Some(data) = STB_RE.captures(size) {
        if let Some(value) = data.get(0) {
            let value = value.as_str().parse::<usize>().unwrap_or(0);
            if value != 0 {
                match data.get(1).expect("Regex capture group doesn't exist?").as_str() {
                    "k" => {
                        return Some(value*KIB)
                    }
                    "m" => {
                        return Some(value*MIB)
                    }
                    "g" => {
                        return Some(value*GIB)
                    }
                    "%" => {
                        return Some(lock.memory_size/value)
                    }
                    _ => {
                        return Some(value)
                    }
                }
            } else {
                return Some(0)
            }
        }
    }
    None
}


fn bytes_to_size(num: usize) -> String {
    if num/GIB > 0 {
        format!("{:.2}g",(num as f32/GIB as f32))
    } else if num/MIB > 0 {
        format!("{:.2}m",(num as f32/MIB as f32))
    } else if num/KIB > 0 {
        format!("{:.2}k",(num as f32/KIB as f32))
    } else {
        format!("{}",num)
    }
}

fn daemon_setup(config_file: &str) -> Result<(), String> {
    let config_data = match fs::read_to_string(config_file) {
        Ok(v) => v,
        Err(err) => return Err(format!("Failed reading {}: {}", config_file, err))
    };
    let config = match toml::from_str::<Table>(&config_data) {
        Ok(v) => v,
        Err(err) => return Err(format!("Failed reading {}: {}", config_file, err))
    };
    
    let sys = System::new_with_specifics(RefreshKind::new().with_memory(MemoryRefreshKind::new().with_ram()));
    let mut lock: Lock = Lock {
        current_size: 0,
        max_file_size: 20*MIB,
        memory_size: sys.total_memory() as usize,
        max_total_size: 0,
        sorting_method: SortingMethod::SL
    };
    lock.max_total_size = lock.memory_size/10;

    // Consume lock config
    let mut files: Vec<DirEntry> = Vec::new();
    if let Some(lock_config) = config["lock"].as_table() {
        lock.max_file_size = size_to_bytes(lock_config["max_file_size"].as_str().unwrap_or("0"), &lock).unwrap_or(lock.max_file_size);
        lock.max_total_size = size_to_bytes(lock_config["max_total_size"].as_str().unwrap_or("0"), &lock).unwrap_or(lock.max_total_size);
        if lock.max_total_size == 0 {
            return Err("Max total size is zero!".to_string())
        }
        if lock.max_file_size == 0 {
            return Err("Max file size is zero!".to_string())
        }

        let locations = lock_config["locations"].as_array().expect("locations was not an array!");
        for location in locations {
            let location = location.as_str().expect("locations have to be strings!");
            if let Ok(location_data) = fs::metadata(location) {
                if location_data.is_dir() {
                    if let Ok(location) = fs::read_dir(location) {
                        for file in location {
                            if let Ok(file) = file {
                                if let Ok(file_data) = file.metadata() {
                                    if file_data.is_file() && file_data.len() as usize <= lock.max_file_size {
                                        files.push(file);
                                    }
                                }
                            }
                        }
                    } else {
                        return Err(format!("Couldn't read {}", location))
                    }
                }
            }
        }

        if let Some(sorting_method) = lock_config["sorting_method"].as_str() {
            match sorting_method.to_lowercase().as_str() {
                "fl" => {
                    println!("Locking in order of first to last");
                    lock.sorting_method = SortingMethod::FL;
                }
                "ls" => {
                    println!("Locking in order of largest to smallest");
                    lock.sorting_method = SortingMethod::LS;
                }
                _ => {
                    println!("Locking in order of smallest to largest");
                    lock.sorting_method = SortingMethod::SL;
                }
            }
        }
    } else {
        return Err(format!("lock table in {} is invalid!", config_file))
    }
    
    let mut to_load: Vec<(String, FileInfo)> = Vec::new();

    // Find specified files
    if let Some(load) = config["load"].as_table() {
        let mut patterns = Vec::new();
        if let Some(files) = load["files"].as_array() {
            for pattern in files {
                patterns.push(pattern.as_str().expect("patterns need to be strings!"));
            }
        }

        if let Some(lists) = load["lists"].as_array() {
            for list in lists {
                let list_id = list.as_str().expect("list needs to be a string!");
                if let Some(list) = load[list_id].as_array() {
                    for pattern in list {
                        patterns.push(pattern.as_str().expect(format!("patterns in {} need to be strings!", list_id).as_str()));
                    }
                }
            }
        } else {
            return Err(format!("load table in {} is invalid!", config_file))
        }
            
        for pattern in patterns {
            let re = RegexBuilder::new(format!(r"/{}\z",pattern).as_str()).size_limit(u16::MAX as usize).build().expect("Unable to build regex pattern");
            for file in files.iter() {
                if let Some(path) = file.path().to_str() {
                    if re.is_match(path) {
                        match file.metadata() {
                            Ok(file_data) => to_load.push((String::from(path), FileInfo { size: file_data.len() })),
                            Err(err) => println!("Unable to get metadata for {}: {}", path, err)
                        }
                    }
                }
            }
        }
        files.clear();
    }

    match lock.sorting_method {
        SortingMethod::SL => to_load.sort_by(|file_a, file_b| file_a.1.size.cmp(&file_b.1.size)),
        SortingMethod::LS => to_load.sort_by(|file_a, file_b| file_b.1.size.cmp(&file_a.1.size)),
        _ => {}
    }
    for to_load in to_load {
        if lock.current_size+(to_load.1.size as usize) > lock.max_total_size {
            continue;
        }
        let path = to_load.0.clone();
        if let Ok(file) = fs::File::open(&path) {
            unsafe {
                // TODO: Stop relying on the default behavior of Mmap::map
                if let Ok(mmap) = Mmap::map(&file) {
                    mmap.lock().expect("Failed to lcoked memory");
                    lock.current_size += mmap.len();
                    LOADED.push((path, mmap));
                } else {
                    println!("Failed to map {} to memory", path);
                }
            }
        }
    }

    unsafe {
        println!("{} of memory, {} files locked", bytes_to_size(lock.current_size), LOADED.len());
    }
    Ok(())
}

fn daemon_run() {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(30));
    }
}

fn daemon_usage() {
    unsafe {
        for file in LOADED.iter() {
            println!("{} - {}", file.0.clone(), bytes_to_size(file.1.len()));
        }
    }
}

fn main() -> Result<(), String> {
    let args = Args::parse();
    let config_file = args.config.unwrap_or(String::from("/etc/prelockd-rs.toml"));
    daemon_setup(config_file.as_str())?;
    if args.usage {
        daemon_usage();
    } else {
        daemon_run();
    }
    Ok(())
}
