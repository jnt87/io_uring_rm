use io_uring::{
    IoUring,
    opcode,
    types,
};
use std::{
    ffi::{CString, CStr, c_char},
    process,
    path::Path,
    fs::OpenOptions,
    os::unix::io::AsRawFd,
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    thread,
    time::Duration,
};
use signal_hook::iterator::Signals;
use libc::{dirent64, AT_REMOVEDIR, AT_FDCWD, O_RDONLY, unlinkat, access, F_OK, DT_DIR, DT_REG}; //want to add O_TRUNC as a fast mode if we
                                                        //think we will replace files

use walkdir::{WalkDir, DirEntry};
use std::path::PathBuf;
use std::fs;
use std::collections::VecDeque;
use std::io;

use clap::{Parser};

#[derive(Parser, Default,Debug)]
#[command(name = "rm", about = "removal in chunks")]
struct Arguments {
    root: String,

    #[arg(short, long, default_value_t = 5)]
    batch_size: usize,
}



struct DirectoryWalker {
    walker: walkdir::IntoIter,
    directories: VecDeque<PathBuf>,
    restricted_files: Vec<PathBuf>,
    restricted_dirs: Vec<PathBuf>,
}

impl DirectoryWalker {
    fn new(root: &str) -> Self {
        DirectoryWalker {
            walker: WalkDir::new(root).into_iter(),
            directories: VecDeque::new(),
            restricted_files: Vec::new(),
            restricted_dirs: Vec::new(),
        }
    }

    fn next_chunk(&mut self, chunk_size: usize) -> Vec<PathBuf> {
        let mut chunk = Vec::new();
        let mut count = 0;
        while count < chunk_size {
            if let Some(Ok(entry)) = self.walker.next() {
                let path = entry.path().to_path_buf();
                
                match fs::metadata(&path) {
                    Ok(metadata) => {
                        if metadata.is_dir() {
                            self.directories.push_front(path);
                        } else if metadata.is_file() {
                            chunk.push(path);
                            count += 1;
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                        if path.is_dir() {
                            self.restricted_dirs.push(path);
                        } else {
                            self.restricted_files.push(path);
                        }
                    }
                    Err(_) => {}
                }
            } else {
                break;
            }
        }
        chunk
    }

    fn next_dir_chunk(&mut self, chunk_size: usize) -> Vec<PathBuf> {
        self.directories.drain(..chunk_size.min(self.directories.len())).collect()
    }

    fn get_directories(&self) -> Vec<PathBuf> {
        self.directories.iter().cloned().collect()
    }
    fn get_restricted_files(&self) -> Vec<PathBuf> {
        self.restricted_files.clone()
    }
    fn get_restricted_dirs(&self) -> Vec<PathBuf> {
        self.restricted_dirs.clone()
    }



}
macro_rules! trust_me_bro {
    ($($stmt:stmt;)*) => {
        unsafe {
            $($stmt)*
        }
    };
}


fn list_dir_entries(path: &str) -> io::Result<Vec<String>> {
    let mut entries = Vec::new();

    for entry in fs::read_dir(Path::new(path))? {
        let entry = entry?;
        let path_buf = entry.path();
        if let Some(path_str) = path_buf.to_str() {
            println!("entry: {}", path_str.to_string());
            entries.push(path_str.to_string());
        }
    }
    Ok(entries)
}

fn delete_directory_iteratively(root_path: &str, ring: &mut IoUring) {
    let mut queue = Box::new(VecDeque::new());
    queue.push_front(root_path.to_string());
    let mut file_deletions: Vec<String> = Vec::new();
    let mut dir_deletions: VecDeque<String> = VecDeque::new();
    let mut sqe_storage_file: Vec<io_uring::squeue::Entry> = Vec::new();
    let mut sqe_storage_dir: VecDeque<io_uring::squeue::Entry> = VecDeque::new();
    let mut path_storage: Vec<CString> = Vec::new();

    if Path::new(&root_path).is_dir() {
        dir_deletions.push_front(root_path.to_string());
    }


    while let Some(path) = queue.pop_front() {
        println!("Scanning path: {}", path.to_string());
        match list_dir_entries(&path) {
            Ok(entries) => {
                if entries.is_empty() {
                    println!("scanned dir is empty, adding for deletion");
                    dir_deletions.push_front(path.clone());
                    continue;
                }
                for entry in entries {
                    match std::fs::metadata(&entry) {
                        Ok(metadata) => {
                            if metadata.is_dir() {
                                println!("Adding dir: {} to be checked", entry);
                                queue.push_front(entry.clone());
                            } else {
                                println!("Adding file: {} to be deleted", entry);
                                file_deletions.push(entry.clone());
                            }
                        }
                        Err(e) => {
                            eprintln!("Warning: Could not access '{}': {}", entry, e);
                        }
                    }
                }
            }
            Err(err) => {
                eprintln!("Failed to list '{}': {}", path, err);
            }
        }
    }
    println!("files to be deleted: {:?}", file_deletions);

    if !file_deletions.is_empty() {
        {
            let mut counter = 0;
            for file in file_deletions {
                let c_file = match CString::new(file.clone()) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("Failed to convert path to CString: {:?}", file);
                        continue;
                    }
                };
                path_storage.push(c_file);
                let c_file_ref = path_storage.last().unwrap();
                let exists_in_rust = Path::new(&file.clone()).exists();
                let c_ptr: *const c_char = c_file_ref.as_ptr();
                let exists_in_c = unsafe { access(c_ptr, F_OK) == 0};
                if exists_in_rust && exists_in_c {
                    println!("file exists in both");
                } else if exists_in_rust {
                    println!("CFile does not exit");
                } else {
                    println!("file does not exist at all");
                }

                let entry = opcode::UnlinkAt::new(types::Fd(AT_FDCWD), c_file_ref.as_ptr())
                    .build()
                    .user_data(counter);
                counter += 1;


                println!("Adding to storage");
                sqe_storage_file.push(entry);
                println!("Submitting request Unlinking {:?}", file);
            }
        }

        let mut sq = ring.submission();
        for entry in &sqe_storage_file {
            println!("submitting stored request");
            unsafe { let _ = sq.push(entry); }
        }
        drop(sq);
        let sub = ring.submit_and_wait(1).expect("submit and wait failed");
        drop(sub);

        let cq = ring.completion();

        for cqe in cq {
            let res = cqe.result();
            if res < 0 {
                eprintln!("Unlink failed with error: {}", -res);
            } else {
                println!("File deleted successfully!");
            }
        }
    }
    
    if !dir_deletions.is_empty() {
        {
            let mut counter = 0;
            for dir in dir_deletions.into_iter().rev() {
                let c_dir = match CString::new(dir.clone()) {
                    Ok(c) => c,
                    Err(_) => {
                        println!("Failed to convert path ot CString: {:?}", dir);
                        continue;
                    }
                };
                path_storage.push(c_dir);
                let c_dir_ref = path_storage.last().unwrap();
                let entry = opcode::UnlinkAt::new(types::Fd(AT_FDCWD), c_dir_ref.as_ptr())
                    .flags(AT_REMOVEDIR)
                    .build()
                    .user_data(counter);
                counter += 1;
                println!("Adding dir to storage");
                sqe_storage_dir.push_front(entry);
                println!("Submitting request Removing directory {:?}", dir);
            }
            let mut sq = ring.submission();
            for entry in &sqe_storage_dir {
                println!("submitting stored dir request");
                unsafe { let _ = sq.push(entry); }
            }
            drop(sq);
            let sub = ring.submit_and_wait(1).expect("submit and wait failed");
            drop(sub);
            let cq = ring.completion();
            for cqe in cq {
                let res = cqe.result();
                if res < 0 {
                    eprintln!("Unlink failed with error: {}", -res);
                } else {
                    println!("File deleted successfully!");
                }
            }
        }
    }
}

fn handle_signals(signals: Vec<i32>, running: Arc<AtomicBool>) {
    let mut signals = Signals::new(&signals).expect("Failed to register signals");

    thread::spawn(move || {
        for signal in signals.forever() {
            println!("Recieved signal: {}", signal);
            running.store(false, Ordering::Relaxed);
            break;
        }
    });

}

fn wait_for_io_uring(ring: &mut IoUring, running: &Arc<AtomicBool>) {
    while running.load(Ordering::Relaxed) {
        println!("wait_for_io_uring while loop");
        /*if ring.submission().is_empty() {
            println!("ring was empty");
            let timespec = types::Timespec::from(Duration::from_secs(3));
            let timeout_entry = opcode::Timeout::new(&timespec).build().user_data(999);

            {
                let mut sq = ring.submission();
                unsafe {
                    let _ = sq.push(&timeout_entry);
                }
                drop(sq);
            }
        }*/

        match ring.submit_and_wait(1) {
            Ok(_) => {
                let mut cq = ring.completion();
                let mut found = false;
                while let Some(cqe) = cq.next() {
                    found = true;
                    println!("submit and wait while loop");
                    if cqe.user_data() == 999 {
                        println!("No pending operations, exiting.");
                        return;
                    }
                    if cqe.result() < 0 {
                        println!("User data: {} Result: {}", cqe.user_data(), cqe.result());
                        println!("Error: {}", std::io::Error::from_raw_os_error(-cqe.result()));
                    }
                }
                if !found {
                    println!("No completion events received, breaking loop.");
                    break;
                }
            }
            Err(e) => {
                eprintln!("Error waiting for completion: {}", e);
                break;
            }
        }
    }

    if !running.load(Ordering::Relaxed) {
        eprintln!("\nOperation interrupted. Exiting.");
        process::exit(130);
    }
}

fn main() {
    println!("started tree parsing");
    let args = Arguments::parse();
    let root: &str = &args.root;
    let mut walker = DirectoryWalker::new(root);
    let chunk_size = args.batch_size;

    loop {
        let files = walker.next_chunk(chunk_size);
        if files.is_empty() {
            println!("Traversal complete.");
            break;
        }

        println!("\nProcessing chunk:");
        for file in files {
            println!("{}", file.display());
        }

        println!("Pausing... Press Entry to continue.");
        let _ = std::io::stdin().read_line(&mut String::new());
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

        println!("Pausing... Press Entry to continue.");
        let _ = std::io::stdin().read_line(&mut String::new());
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

    println!("Started rm");

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <file>", args[0]);
        process::exit(1);
    }

    let mut path = root;
    let mut c_path;
    let metadata = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) => {
            eprintln!("Failed to access '{}': {}", path, err);
            process::exit(1);
        }
    };
    
    let mut ring = IoUring::new(8).expect("Failed to create io_uring");

    println!("created io_uring");
    let running = Arc::new(AtomicBool::new(true));
    let signals = vec![libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

    handle_signals(signals, running.clone());
    if metadata.is_dir() {
        delete_directory_iteratively(path, &mut ring);
        println!("finished delete_directory_iteratively");
        process::exit(1);
    } else {
        let vector = match list_dir_entries(path) {
            Ok(vec) => vec,
            Err(e) => {
                eprintln!("Error occurred: {}", e);
                return;
            }
        };
        println!("just a file...");
        println!("check1: {}", path.to_string());
        println!("check2: {:?}", vector);
        if let Some(index) = vector.iter().position(|s| s == path) {
            path = vector.get(index).unwrap();
            println!("path to string from vec: {}", path.to_string());
        } else {
            println!("found strings not valid");
            process::exit(1);
        }
        c_path = CString::new(path.to_string()).unwrap(); //bad
        let entry = opcode::UnlinkAt::new(types::Fd(libc::AT_FDCWD), c_path.as_ptr())
            .build()
            .user_data(42);
        let mut sq = ring.submission();
        unsafe {
            sq.push(&entry).expect("Submission queue is full");
        }
        drop(sq);
    }
    //wait_for_io_uring(&mut ring, &running);
/*    ring.submit_and_wait(1);
    let cq = ring.completion();

    for cqe in cq {
        if cqe.user_data() == 42 {
            println!("user data match");
            let res = cqe.result();
            if res < 0 {
                eprintln!("Unlink failed with error: {}", -res);
            } else {
                println!("Filed deleted sucessfully");
            }
        }
    }*/
/*    let c_path = CString::new(path.to_string()).unwrap();

    let unlink_e = opcode::UnlinkAt::new(types::Fd(libc::AT_FDCWD), c_path.as_ptr())
        .build()
        .user_data(42);

    let mut sq = ring.submission();

    unsafe {
        sq.push(&unlink_e).expect("Submission queue is full");
    }
    drop(sq);
*/
    println!("Forward progress");
    ring.submit_and_wait(1).expect("submit and wait failed");

    let cq = ring.completion();

    for cqe in cq {
        let res = cqe.result();
        if res < 0 {
            eprintln!("Unlink failed with error: {}", -res);
        } else {
            println!("File deleted successfully!");
        }
    }
}
