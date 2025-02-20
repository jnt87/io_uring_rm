use io_uring::{
    IoUring,
    opcode,
    types,
};
use std::{
    collections::VecDeque,
    ffi::{CString, CStr},
    process,
    path::Path,
    fs,
    fs::OpenOptions,
    io,
    os::unix::io::AsRawFd,
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    thread,
    time::Duration,
};
use signal_hook::iterator::Signals;
use libc::{dirent64, AT_REMOVEDIR, AT_FDCWD, O_RDONLY}; //want to add O_TRUNC as a fast mode if we
                                                        //think we will replace files

macro_rules! trust_me_bro {
    ($($stmt:stmt;)*) => {
        unsafe {
            $($stmt)*
        }
    };
}

fn list_dir2(path: &str) -> io::Result<Vec<String>> {
    let mut entries = Vec::new();

    for entry in fs::read_dir(Path::new(path))? {
        let entry = entry?;
        let path = entry.path();
        if let Some(name) = path.file_name() {
            if let Some(name_str) = name.to_str() {
                entries.push(name_str.to_string());
            }
        }
    }
    Ok(entries)
}

fn list_dir(path: &str) -> io::Result<Vec<String>> {
    let dir = OpenOptions::new().read(true).open(path)?;
    let fd = dir.as_raw_fd();
    let mut buf = vec![0; 4096];
    let mut entries = Vec::new();
    unsafe {
        let nread = libc::syscall(libc::SYS_getdents64, fd, buf.as_mut_ptr(), buf.len()) as isize;
        if nread < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut offset = 0;
        while offset < nread as usize { 
            println!("list_dir while loop");
            let d = &*(buf.as_ptr().add(offset) as *const dirent64);

            if d.d_reclen == 0 {
                break;
            }
            let name_bytes = &d.d_name;
            let name_cstr = CStr::from_ptr(name_bytes.as_ptr().cast());
            let name = name_cstr.to_string_lossy().into_owned();
            println!("Name: {}", name);
            if name != "." && name != ".." {
                entries.push(format!("{}/{}", path, name));
            }
            offset += d.d_reclen as usize;
            println!("offset: {} and d_reclen: {}", offset, d.d_reclen);
        }
    };
    Ok(entries)
}

fn delete_directory_iteratively(root_path: &str, ring: &mut IoUring) {
    let mut stack = VecDeque::new();
    stack.push_back(root_path.to_string());

    let mut file_deletions: Vec<String> = Vec::new();
    let mut dir_deletions: Vec<String> = Vec::new();
    println!("Path: {}", root_path);
    while let Some(path) = stack.pop_back() {
        match list_dir2(&path) {
            Ok(entries) => {
                let mut has_subdirs = false;
                for entry in entries {
                    let metadata = std::fs::metadata(&entry).unwrap();
                    if metadata.is_dir() {
                        println!("Adding dir: {} to be checked", entry);
                        stack.push_back(entry);
                        has_subdirs = true;
                    } else {
                        println!("Adding file: {} to be deleted", entry);
                        file_deletions.push(entry);
                    }
                }
                if !has_subdirs {
                    println!("Adding file: {} to be deleted", path);
                } else {
                    stack.push_back(path);
                }
            }
            Err(err) => {
                eprintln!("Failed to list '{}': {}", path, err);
            }
        }
    }
    {
        let mut sq = ring.submission();
        for file in file_deletions {
            let c_file = CString::new(file.as_bytes()).unwrap(); //bad
            let entry = opcode::UnlinkAt::new(types::Fd(AT_FDCWD), c_file.as_ptr())
                .build()
                .user_data(0);
            println!("Submitting request Unlinking {}", file);
            unsafe {
                let _ = sq.push(&entry); //dont ignore
            }
        }
    }
    {
        let mut sq = ring.submission();
        for dir in dir_deletions.into_iter().rev() {
            let c_dir = CString::new(dir.as_bytes()).unwrap();
            let entry = opcode::UnlinkAt::new(types::Fd(AT_FDCWD), c_dir.as_ptr())
                .flags(AT_REMOVEDIR)
                .build()
                .user_data(1);
            println!("Submitting request Removing directory {}", dir);
            unsafe {
                let _ = sq.push(&entry);
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
        let timespec = types::Timespec::from(Duration::from_secs(2));
        let timeout_entry = opcode::Timeout::new(&timespec).build().user_data(999);

        {
            let mut sq = ring.submission();
            unsafe {
                let _ = sq.push(&timeout_entry);
            }
        }

        match ring.submit_and_wait(1) {
            Ok(_) => {
                let mut cq = ring.completion();
                while let Some(cqe) = cq.next() {
                    println!("submit and wait while loop");
                    if cqe.user_data() == 999 {
                        println!("No pending operations, exiting.");
                        return;
                    }
                    if cqe.result() < 0 {
                        eprintln!("Error: {}", std::io::Error::from_raw_os_error(-cqe.result()));
                    }
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
    println!("Started rm");
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <file>", args[0]);
        process::exit(1);
    }

    let path = &args[1];

    let metadata = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) => {
            eprintln!("Failed to access '{}': {}", path, err);
            process::exit(1);
        }
    };
    
    if metadata.is_dir() {
        let entries = list_dir2(path).unwrap();
        println!("Entries: {:?}", entries);
    }

    println!("Checked arg");

    let mut ring = IoUring::new(8).expect("Failed to create io_uring");

    println!("created io_uring");

    let running = Arc::new(AtomicBool::new(true));
    let signals = vec![libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

    handle_signals(signals, running.clone());
    
    if metadata.is_dir() {
        delete_directory_iteratively(path, &mut ring);
    } else {
        let c_path = CString::new(path.as_bytes()).unwrap(); //bad
        let entry = opcode::UnlinkAt::new(types::Fd(AT_FDCWD), c_path.as_ptr())
            .build()
            .user_data(7);
        let mut sq = ring.submission();
        unsafe {
            let _ = sq.push(&entry);
        }
    }
    wait_for_io_uring(&mut ring, &running);

}
