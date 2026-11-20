// Benchmark runner - calls the Naso-compiled naso_entry function

extern "C" {
    fn naso_entry();
}

fn main() {
    println!("Starting bench_runner...");

    // Run naso_entry in a separate thread with larger stack (8MB)
    let handle = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024) // 8MB stack
        .spawn(|| {
            println!("Running naso_entry on thread with 8MB stack...");
            unsafe { naso_entry(); }
        })
        .expect("Failed to spawn thread");

    handle.join().expect("naso_entry thread panicked");

    loop {
        println!("Waiting for connections...");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}