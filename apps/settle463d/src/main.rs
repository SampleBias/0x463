fn main() -> Result<(), Box<dyn std::error::Error>> {
    settle463d::run_cli(&std::env::args().skip(1).collect::<Vec<_>>())
}
