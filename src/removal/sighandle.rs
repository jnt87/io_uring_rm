use signal_hook::iterator::Signals;
use std::thread;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

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
