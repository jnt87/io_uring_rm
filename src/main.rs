use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
use clap::{Parser};

use io_uring_rm::removal::{Arguments, DirectoryWalker, IoUringRm, handle_signals};
use rand::SeedableRng;
use rand_chacha::ChaChaRng;
use random_tree::{create_random_tree};

fn main() {
    //println!("started tree parsing");
    let args = Arguments::parse();
    let root: &str = &args.root;
    let confirm = args.confirm.clone();
    let seed = 7;
    //let mut rng = ChaChaRng::seed_from_u64(seed);
    //create_random_tree(&PathBuf::from(root), &mut rng, 3);
    let mut walker = DirectoryWalker::new(root);
    let chunk_size = args.batch_size;
    let mut rmer = IoUringRm::new(chunk_size as u32).expect("Failed to create io_uring");

    loop {
        let files = walker.next_chunk(chunk_size);
        if files.is_empty() {
            //println!("Traversal complete.");
            break;
        }

        //println!("\nProcessing chunk:");
        for file in &files {
            //println!("{}", file.display());
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
            //println!("Traversal complete.");
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

    for file in walker.get_restricted_files() {
        println!("Restricted file: {}", file.display());
    }

    for dir in walker.get_restricted_dirs() {
        println!("Restricted dir: {}", dir.display());
    }

    //println!("ended tree parsing");

    let running = Arc::new(AtomicBool::new(true));
    let signals = vec![libc::SIGINT, libc::SIGTERM, libc::SIGHUP];
    handle_signals(signals, running.clone());
}

// simple unit tests to confirm the rm function works on a single file
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_rm() {
        let root = PathBuf::from("/tmp");
        let mut walker = DirectoryWalker::new(&root);
        let files = walker.next_chunk(1);
        assert_eq!(files.len(), 1);
        let mut rmer = IoUringRm::new(1).expect("Failed to create io_uring");
        rmer.delete_files(files);
    }
}
