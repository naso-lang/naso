// Rust wrapper that calls the Naso-compiled naso_entry function

// Declare the naso_entry function from the Naso-compiled object file
extern "C" {
    fn naso_entry();
}

fn main() {
    // Call the Naso entry point
    unsafe {
        naso_entry();
    }
}