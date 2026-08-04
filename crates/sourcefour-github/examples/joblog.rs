//! Hand-run log-download check:
//! `gh auth token | cargo run -p sourcefour-github --example joblog -- <job id>`
use std::io::Read as _;

fn main() {
    let job_id: u64 = std::env::args()
        .nth(1)
        .expect("job id")
        .parse()
        .expect("numeric");
    let mut token = String::new();
    std::io::stdin()
        .read_to_string(&mut token)
        .expect("token on stdin");
    let remote =
        sourcefour_github::GithubRemote::parse("git@github.com:HelgeSverre/sourcefour.git")
            .expect("parses");
    match sourcefour_github::job_log(
        &sourcefour_github::UreqTransport,
        &remote,
        token.trim(),
        job_id,
    ) {
        Ok(log) => println!(
            "OK {} bytes, first line: {}",
            log.len(),
            log.lines().next().unwrap_or("")
        ),
        Err(error) => {
            eprintln!("ERR {error}");
            std::process::exit(1);
        }
    }
}
