// Benchmark runner - calls the Naso-compiled naso_entry function

extern "C" {
    fn naso_entry();
}

fn main() {
    println!("Starting bench_runner...");

    // Call the generated Naso entry point
    unsafe { naso_entry(); }

    loop {
        println!("Waiting for connections...");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}