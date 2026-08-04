//! A hand-run connection check through the shipping transport:
//!
//!   gh auth token | cargo run -p sourcefour-github --example whoami
//!
//! Reads the token from stdin so it never lands in shell history or argv.

use std::io::Read as _;

fn main() {
    let mut token = String::new();
    std::io::stdin()
        .read_to_string(&mut token)
        .expect("a token arrives on stdin");
    let token = token.trim();
    assert!(
        !token.is_empty(),
        "pipe a token in, e.g. `gh auth token | …`"
    );

    match sourcefour_github::whoami(&sourcefour_github::UreqTransport, token) {
        Ok(account) => println!("connected as {}", account.login),
        Err(failure) => {
            eprintln!("{failure}");
            std::process::exit(1);
        }
    }
}
