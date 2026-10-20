fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = anchor_hub::adapters::run(&args) {
        eprintln!("{error}");
        std::process::exit(2);
    }
}
