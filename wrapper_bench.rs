// Rust wrapper that calls the Naso-compiled naso_entry function

extern "C" {
    fn naso_entry();
}

fn main() {
    unsafe {
        naso_entry();
    }
}