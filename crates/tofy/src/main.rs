fn main() {
    if let Err(e) = tofy::cli::run() {
        eprintln!("tofy: {e}");
        std::process::exit(e.exit_code());
    }
}
