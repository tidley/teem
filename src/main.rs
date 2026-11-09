fn main() {
    // SAFETY: this is called before threads are created; all Teem state stays owner-only.
    unsafe {
        libc::umask(0o077);
    }
    if let Err(error) = teem::plugin::main(std::env::args().skip(1).collect()) {
        eprintln!("{error}");
        std::process::exit(2);
    }
}
