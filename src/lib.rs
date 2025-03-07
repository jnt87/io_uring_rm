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

pub mod removal {
    pub mod arguments;
    pub use arguments::Arguments;
}

pub struct IoUringRm {
    ring: IoUring,
    path_storage: Vec<CString>,
}

impl IoUringRm {
    pub fn new(depth: u32) -> io::Result<Self> {
        let ring = IoUring::new(depth)?;
        Ok(Self {
            ring,
            path_storage: Vec::new(),
        })
    }

    pub fn delete_files(&mut self, files: Vec<PathBuf>) {
        let mut sqe_storage: Vec<io_uring::squeue::Entry> = Vec::new();

        for file in files {
            if let Ok(c_file) = CString::new(file.to_string_lossy().as_bytes()) {
                self.path_storage.push(c_file);
                if let Some(c_file_ref) = self.path_storage.last() {
    
                    let entry = opcode::UnlinkAt::new(types::Fd(AT_FDCWD), c_file_ref.as_ptr())
                        .build()
                        .user_data(sqe_storage.len() as u64);
    
                    sqe_storage.push(entry);
                } else {
                    eprintln!("Error: Failed to get last stored poath for {:?}", file);
                }
            } else {
                eprintln!("Failed to convert path: {:?}", file);
            }
        }
        self.submit_and_wait(&sqe_storage);
    }

    pub fn delete_directories(&mut self, dirs: Vec<PathBuf>) {
        let mut sqe_storage: Vec<io_uring::squeue::Entry> = Vec::new();

        for dir in dirs {
            if let Ok(c_dir) = CString::new(dir.to_string_lossy().as_bytes()) {
                self.path_storage.push(c_dir);
                if let Some(c_dir_ref) = self.path_storage.last() {
    
                    let entry = opcode::UnlinkAt::new(types::Fd(AT_FDCWD), c_dir_ref.as_ptr())
                        .flags(AT_REMOVEDIR)
                        .build()
                        .user_data(sqe_storage.len() as u64);
    
                    sqe_storage.push(entry);
                } else {
                    eprintln!("Error: Failed to get last stored path for {:?}", dir);
                }
            } else {
                eprintln!("Failed to convert path: {:?}", dir);
            }

        }
        self.submit_and_wait(&sqe_storage);
    }

    pub fn submit_and_wait(&mut self, entries: &[io_uring::squeue::Entry]) {
        let mut sq = self.ring.submission();
        for entry in entries {
            unsafe {
                let _ = sq.push(entry);
            }
        }
        drop(sq);

        self.ring.submit_and_wait(entries.len()).expect("Submission failed");

        let cq = self.ring.completion();
        for cqe in cq {
            let res = cqe.result();
            if res < 0 {
                eprintln!("Unlink failed with error: {}", -res);
            } else {
                println!("Deletion successful");
            }
        }
    }
}



pub struct DirectoryWalker {
    walker: walkdir::IntoIter,
    directories: VecDeque<PathBuf>,
    restricted_files: Vec<PathBuf>,
    restricted_dirs: Vec<PathBuf>,
}

impl DirectoryWalker {
    pub fn new(root: &str) -> Self {
        DirectoryWalker {
            walker: WalkDir::new(root).into_iter(),
            directories: VecDeque::new(),
            restricted_files: Vec::new(),
            restricted_dirs: Vec::new(),
        }
    }

    pub fn next_chunk(&mut self, chunk_size: usize) -> Vec<PathBuf> {
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

    pub fn next_dir_chunk(&mut self, chunk_size: usize) -> Vec<PathBuf> {
        self.directories.drain(..chunk_size.min(self.directories.len())).collect()
    }

    pub fn get_directories(&self) -> Vec<PathBuf> {
        self.directories.iter().cloned().collect()
    }
    pub fn get_restricted_files(&self) -> Vec<PathBuf> {
        self.restricted_files.clone()
    }
    pub fn get_restricted_dirs(&self) -> Vec<PathBuf> {
        self.restricted_dirs.clone()
    }



}

pub fn handle_signals(signals: Vec<i32>, running: Arc<AtomicBool>) {
    let mut signals = Signals::new(&signals).expect("Failed to register signals");

    thread::spawn(move || {
        for signal in signals.forever() {
            println!("Recieved signal: {}", signal);
            running.store(false, Ordering::Relaxed);
            break;
        }
    });

}

