use io_uring::{
    IoUring,
    opcode,
    types,
};
use std::{
    ffi::{CString, c_char},
    process,
    path::{Path, PathBuf},
    fs::OpenOptions,
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    thread,
    time::Duration,
    fs,
    collections::VecDeque,
    io,
};
use signal_hook::iterator::Signals;
use libc::{AT_REMOVEDIR, AT_FDCWD, access, F_OK}; 
use walkdir::WalkDir;
use clap::{Parser};

use io_uring_rm::{IoUringRm, handle_signals};
use io_uring_rm::removal::{Arguments, DirectoryWalker};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaChaRng;
use random_tree::{create_random_tree};

fn main() {
    println!("started tree parsing");
    let args = Arguments::parse();
    let root: &str = &args.root;
    let confirm = args.confirm.clone();
    let seed = 7;
    let mut rng = ChaChaRng::seed_from_u64(seed);
    create_random_tree(&PathBuf::from(root), &mut rng, 3);
    let mut walker = DirectoryWalker::new(root);
    let chunk_size = args.batch_size;
    let mut rmer = IoUringRm::new(chunk_size as u32).expect("Failed to create io_uring");

    loop {
        let files = walker.next_chunk(chunk_size);
        if files.is_empty() {
            println!("Traversal complete.");
            break;
        }

        println!("\nProcessing chunk:");
        for file in &files {
            println!("{}", file.display());
        }
        if confirm {
            println!("Pausing... Press Entry to continue.");
            let _ = std::io::stdin().read_line(&mut String::new());
        }
        rmer.delete_files(files);
    }
    loop {
        let dirs = walker.next_dir_chunk(chunk_size);
        if dirs.is_empty() {
            println!("Traversal complete.");
            break;
        }

        println!("\nProcessing chunk:");
        for dir in &dirs {
            println!("{}", dir.display());
        }

        if confirm {
            println!("Pausing... Press Entry to continue.");
            let _ = std::io::stdin().read_line(&mut String::new());
        }
        rmer.delete_directories(dirs);
    }   

    println!("\nRestricted files (no permissions):");
    for file in walker.get_restricted_files() {
        println!("{}", file.display());
    }

    println!("\nRestricted directories (no permissions):");
    for dir in walker.get_restricted_dirs() {
        println!("{}", dir.display());
    }

    println!("ended tree parsing");

    let running = Arc::new(AtomicBool::new(true));
    let signals = vec![libc::SIGINT, libc::SIGTERM, libc::SIGHUP];
    handle_signals(signals, running.clone());
}
