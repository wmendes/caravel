//! `caravel`: the Caravel CLI. Installed as `stellar-caravel`, it is also a
//! Stellar CLI plugin (`stellar caravel plan`).

fn main() -> std::process::ExitCode {
    caravel_cli::main()
}
