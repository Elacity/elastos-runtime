fn main() {
    let paths = std::env::args_os().skip(1).collect::<Vec<_>>();
    if paths.first().is_some_and(|path| path == "--child") {
        return;
    }
    for path in paths {
        if std::fs::canonicalize(path).is_err() {
            std::process::exit(1);
        }
    }
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--child")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(mut child) = result else {
        std::process::exit(2);
    };
    if !child.wait().is_ok_and(|status| status.success()) {
        std::process::exit(2);
    }
}
