// P3 wires these into the command tree.
#[allow(dead_code)]
mod detach;
#[allow(dead_code)]
mod gitscope_helper;
#[allow(dead_code)]
mod wait;

fn main() {
    // Go main: load .env silently, then the stderr JSON logger.
    // SAFETY: first statement of main; no other thread exists yet.
    unsafe { rival_core::paths::load_dotenv() };
    rival_core::logging::init();

    match std::env::args().nth(1).as_deref() {
        Some("version") => println!("{}", rival_core::version_line()),
        _ => {
            eprintln!("usage: rival version");
            std::process::exit(2);
        }
    }
}
