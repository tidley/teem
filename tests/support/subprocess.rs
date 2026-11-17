//! Native child processes used by integration tests. Never installed by the plugin.
use std::{
    io::Write,
    process::{Command, Stdio},
    time::Duration,
};

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("deleted-runner") => {
            std::fs::remove_file(std::env::current_exe().unwrap()).unwrap();
            let mut runner = teem::checks::Runner::default();
            let handle = runner.start(std::path::Path::new(&args[1]), std::path::Path::new(&args[2]),
                &serde_json::json!([{"name":"inspect","kind":"inspect","argv":["git","status","--porcelain"]}]), "snapshot").unwrap();
            let start = std::time::Instant::now();
            loop {
                if let Some(receipt) = teem::checks::inspect(&handle).unwrap() {
                    assert_eq!(receipt["passed"], true, "{receipt}");
                    break;
                }
                assert!(start.elapsed() < Duration::from_secs(10));
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        Some("check-helper")
            if std::env::current_exe().unwrap().file_name().unwrap()
                == "deleted-runner-fixture (deleted)" =>
        {
            let spec = serde_json::from_slice(&std::fs::read(&args[1]).unwrap()).unwrap();
            teem::checks::run(&spec).unwrap();
        }
        Some("sleep") => std::thread::sleep(Duration::from_millis(args[1].parse().unwrap())),
        Some("output") => {
            let n: usize = args[1].parse().unwrap();
            std::io::stdout().write_all(&vec![b'x'; n]).unwrap();
        }
        Some("closed") => {
            // SAFETY: this fixture intentionally closes only its own output descriptors.
            unsafe {
                libc::close(1);
                libc::close(2);
            }
            std::thread::sleep(Duration::from_millis(args[1].parse().unwrap()));
        }
        Some("background") => {
            // This fixture deliberately exits before its child. The supervisor's
            // process-group cleanup, not this parent, is what the test verifies.
            #[allow(clippy::zombie_processes)]
            let child = Command::new(std::env::current_exe().unwrap())
                .args(["sleep", "60000"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            println!("{}", child.id());
            std::io::stdout().flush().unwrap();
        }
        Some("arguments") => println!("{}", serde_json::to_string(&args[1..]).unwrap()),
        Some("agent") if args == ["agent", "get", "issue1-pane"] => {
            println!("{}", std::env::var("TEEM_FIXTURE_HERDR_RESULT").unwrap());
        }
        Some("pane") if args == ["pane", "list"] => {
            println!("{}", std::env::var("TEEM_FIXTURE_HERDR_ROSTER").unwrap());
        }
        _ => std::process::exit(2),
    }
}
