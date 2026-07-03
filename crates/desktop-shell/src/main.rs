fn main() {
    if let Err(error) = desktop_shell::run_from_env() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
