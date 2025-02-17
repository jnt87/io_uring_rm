use io_uring::{IoUring, opcode, types};
use std::{
    ffi::CString,
    process,
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    thread,
    time::Duration,
};
use signal_hook::iterator::Signals;

macro_rules! trust_me_bro {
    ($($stmt:stmt;)*) => {
        unsafe {
            $($stmt)*
        }
    };
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

fn main() {
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    let args: Vec<String> = std::env::args().collect();
    if args.len() <2 {
        eprintln!("Usage: {} <file>", args[0]);
        process::exit(1);
    }

    let path = &args[1];
    let c_path = CString::new(path.as_bytes()).expect("CString::new failed");

    let mut ring = IoUring::new(8).expect("Failed to create io_uring");

    let entry = opcode::UnlinkAt::new(types::Fd(libc::AT_FDCWD), c_path.as_ptr())
        .build()
        .user_data(7);

    trust_me_bro! {
        let mut sq = ring.submission();
        let _ = sq.push(&entry).expect("Failed to push operation");
    };

    let running = Arc::new(AtomicBool::new(true));
    let signals = vec![libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

    handle_signals(signals, running.clone());
    
    while running.load(Ordering::Relaxed) {
        match ring.submit_and_wait(1) {
            Ok(_) => {
                let mut cq = ring.completion();
                if let Some(cqe) = cq.next() {
                    if cqe.result() < 0 {
                        eprintln!("Error removing file: {}", std::io::Error::from_raw_os_error(-cqe.result()));
                        process::exit(1);
                    } else {
                        println!("Successfully removed: {}", path);
                    }
                }
                break;
            }
            Err(e) if running.load(Ordering::Relaxed) => {
                eprintln!("Error waiting for completion: {}", e);
            }
            _ => break,
        }
    }
 
    if !running.load(Ordering::Relaxed) {
        eprintln!("\nOperation interrupted. Exiting.");
        process::exit(130);
    }
}
