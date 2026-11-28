fn main() {
    if let Err(error) = agentterm_lib::dispatch_bridge::main_stdio() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
