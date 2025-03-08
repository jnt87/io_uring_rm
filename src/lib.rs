use io_uring::{
    IoUring,
    opcode,
    types,
};
use std::{
    ffi::CString,
    path::PathBuf,
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    thread,
    io,
};
use signal_hook::iterator::Signals;
use libc::{AT_REMOVEDIR, AT_FDCWD}; 

pub mod removal {
    pub mod arguments;
    pub mod directorywalker;

    pub use arguments::Arguments;
    pub use directorywalker::DirectoryWalker;
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

