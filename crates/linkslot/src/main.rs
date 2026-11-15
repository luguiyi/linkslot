//! `linkSlot` binary entry point. All logic lives in the library so tests can
//! exercise the CLI without spawning a process.

fn main() {
    std::process::exit(linkslot::cli::run());
}
