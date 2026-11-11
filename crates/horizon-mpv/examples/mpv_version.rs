//! Prints the client API version of a libmpv, opened the way the game opens it.
//!
//! `cargo run -p horizon-mpv --example mpv_version -- <version folder>` checks
//! the library the launcher installed there (`native-libraries.json`);
//! `-- --file <full path to libmpv>` opens a file directly, unchecked.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opened = match args.as_slice() {
        [flag, file] if flag == "--file" => horizon_mpv::Mpv::open(&PathBuf::from(file)),
        [dir] => horizon_mpv::Mpv::open_installed(&PathBuf::from(dir), "libmpv"),
        _ => {
            eprintln!("usage: mpv_version <version folder> | --file <full path to libmpv>");
            return ExitCode::from(2);
        }
    };
    match opened {
        Ok(mpv) => {
            let (major, minor) = mpv.client_api_version();
            println!("mpv_client_api_version: {major}.{minor}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
