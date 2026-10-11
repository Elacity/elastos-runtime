use super::*;
use std::cell::RefCell;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

/// Answers each command by its program and first argument; records every call.
struct FakeManager {
    replies: Vec<(&'static str, &'static str, i32, &'static str)>,
    calls: RefCell<Vec<String>>,
}

impl FakeManager {
    fn new(replies: Vec<(&'static str, &'static str, i32, &'static str)>) -> Self {
        Self {
            replies,
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl Manager for FakeManager {
    fn run(&self, program: &str, args: &[&OsStr]) -> std::io::Result<Output> {
        let words: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
        self.calls
            .borrow_mut()
            .push(format!("{program} {}", words.join(" ")));
        let verb = words
            .iter()
            .find(|word| *word != "--user")
            .cloned()
            .unwrap_or_default();
        let (code, stdout) = self
            .replies
            .iter()
            .find(|(p, v, _, _)| *p == program && *v == verb)
            .map(|(_, _, code, stdout)| (*code, *stdout))
            .unwrap_or((0, ""));
        Ok(Output {
            status: ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: if code == 0 {
                Vec::new()
            } else {
                b"refused\n".to_vec()
            },
        })
    }
}

fn spec(platform: Platform, root: &Path, data: &str) -> Spec {
    Spec {
        platform,
        home: root.join("home"),
        data_dir: root.join(data),
        xdg_config_home: None,
        xdg_data_home: None,
        path: "/usr/bin:/bin".into(),
        lang: None,
    }
}

#[test]
fn systemd_unit_keeps_paths_with_spaces_and_percent_literal() {
    let root = tempfile::tempdir().unwrap();
    let spec = spec(Platform::Linux, root.path(), "data 100%/elastos");
    let unit = spec.render().unwrap();
    let binary = spec.binary().display().to_string().replace('%', "%%");
    assert!(unit.contains(&format!("\nExecStart=\"{binary}\" home --browser\n")));
    assert!(unit.contains("\"ELASTOS_SERVICE=1\""));
    // Directives that need user namespaces can stop Home starting; none are present.
    for directive in [
        "ProtectHome",
        "ProtectSystem",
        "PrivateTmp",
        "PrivateDevices",
        "ReadWritePaths",
    ] {
        assert!(!unit.contains(directive), "{directive}");
    }
}

#[test]
fn definitions_refuse_paths_the_manager_would_misread() {
    let root = tempfile::tempdir().unwrap();
    for platform in [Platform::Linux, Platform::MacOs] {
        for data in [
            "quote\"d/elastos",
            "line\nbreak/elastos",
            "back\\slash/elastos",
        ] {
            assert!(
                spec(platform, root.path(), data).render().is_err(),
                "{data:?}"
            );
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn launch_agent_is_a_valid_property_list_with_exact_arguments() {
    let root = tempfile::tempdir().unwrap();
    let mut spec = spec(Platform::MacOs, root.path(), "Application Support/<el&os>");
    spec.lang = Some("en_US.UTF-8".into());
    let path = root.path().join("agent.plist");
    fs::write(&path, spec.render().unwrap()).unwrap();
    let output = Command::new("plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["Label"], LAUNCHD_LABEL);
    assert_eq!(
        value["ProgramArguments"],
        serde_json::json!([spec.binary().display().to_string(), "home", "--browser"])
    );
    assert_eq!(value["EnvironmentVariables"]["ELASTOS_SERVICE"], "1");
    assert_eq!(value["EnvironmentVariables"]["LANG"], "en_US.UTF-8");
    assert_eq!(value["KeepAlive"]["SuccessfulExit"], false);
    assert_eq!(value["LimitLoadToSessionType"], "Aqua");
    assert_eq!(value["Umask"], 63);
}

#[test]
fn only_the_definition_for_this_data_directory_is_managed() {
    let root = tempfile::tempdir().unwrap();
    for platform in [Platform::Linux, Platform::MacOs] {
        let manager = FakeManager::new(vec![]);
        let mine = Service::new(spec(platform, root.path(), "mine/elastos"), &manager);
        let other = Service::new(spec(platform, root.path(), "other/elastos"), &manager);
        assert!(!mine.installed());
        mine.write_definition().unwrap();
        assert!(mine.installed());
        // Same definition path, another installation's Runtime.
        assert!(!other.installed());
        fs::remove_file(mine.spec.definition()).unwrap();
    }
}

#[test]
fn failed_start_removes_the_definition_so_home_falls_back_cleanly() {
    let root = tempfile::tempdir().unwrap();
    let linux = FakeManager::new(vec![("systemctl", "restart", 1, "")]);
    let service = Service::new(spec(Platform::Linux, root.path(), "data/elastos"), &linux);
    assert!(service.install().is_err());
    assert!(!service.spec.definition().exists());
    assert!(linux
        .calls
        .borrow()
        .iter()
        .any(|call| call == "systemctl --user disable --now elastos-home.service"));

    let mac = FakeManager::new(vec![
        ("launchctl", "bootstrap", 5, ""),
        ("launchctl", "print", 113, ""),
    ]);
    let service = Service::new(spec(Platform::MacOs, root.path(), "data/elastos"), &mac);
    assert!(service.install().is_err());
    assert!(!service.spec.definition().exists());
}

#[test]
fn a_unit_in_auto_restart_counts_as_failed_and_restarts_are_reported() {
    let root = tempfile::tempdir().unwrap();
    let cases = [
        (
            "ActiveState=active\nSubState=running\nNRestarts=0\nResult=success\n",
            State::Running,
            0,
        ),
        (
            "ActiveState=inactive\nSubState=dead\nNRestarts=0\nResult=success\n",
            State::Stopped,
            0,
        ),
        (
            "ActiveState=activating\nSubState=auto-restart\nNRestarts=1\nResult=exit-code\n",
            State::Failed(String::new()),
            1,
        ),
        // A long-running unit that once restarted still runs (an update pauses it);
        // a start that restarted fails its readiness wait instead.
        (
            "ActiveState=active\nSubState=running\nNRestarts=2\nResult=success\n",
            State::Running,
            2,
        ),
        (
            "ActiveState=failed\nSubState=failed\nNRestarts=5\nResult=start-limit-hit\n",
            State::Failed(String::new()),
            5,
        ),
    ];
    for (show, expected, restarts) in cases {
        let manager = FakeManager::new(vec![("systemctl", "show", 0, show)]);
        let service = Service::new(spec(Platform::Linux, root.path(), "data/elastos"), &manager);
        let (state, observed) = service.observe().unwrap();
        assert_eq!(
            std::mem::discriminant(&state),
            std::mem::discriminant(&expected),
            "{show}"
        );
        assert_eq!(observed, restarts, "{show}");
    }
}

#[test]
fn launch_agent_that_exited_with_an_error_is_failed() {
    let root = tempfile::tempdir().unwrap();
    let cases = [
        ("\tstate = running\n\tpid = 42\n", State::Running),
        (
            "\tstate = not running\n\tlast exit code = 0\n",
            State::Stopped,
        ),
        (
            "\tstate = not running\n\tlast exit code = 1\n",
            State::Failed(String::new()),
        ),
        (
            "\tstate = spawn scheduled\n\tlast exit code = 78: EX_CONFIG\n",
            State::Failed(String::new()),
        ),
    ];
    for (print, expected) in cases {
        let manager = FakeManager::new(vec![("launchctl", "print", 0, print)]);
        let service = Service::new(spec(Platform::MacOs, root.path(), "data/elastos"), &manager);
        assert_eq!(
            std::mem::discriminant(&service.state().unwrap()),
            std::mem::discriminant(&expected),
            "{print}"
        );
    }
}

#[test]
fn linger_is_requested_once_and_a_refusal_is_one_sentence() {
    let root = tempfile::tempdir().unwrap();
    let lingering = FakeManager::new(vec![("loginctl", "show-user", 0, "yes\n")]);
    let service = Service::new(
        spec(Platform::Linux, root.path(), "data/elastos"),
        &lingering,
    );
    assert_eq!(service.ensure_linger(), None);
    assert!(!lingering
        .calls
        .borrow()
        .iter()
        .any(|call| call.starts_with("loginctl enable-linger")));

    let refused = FakeManager::new(vec![
        ("loginctl", "show-user", 0, "no\n"),
        ("loginctl", "enable-linger", 1, ""),
    ]);
    let service = Service::new(spec(Platform::Linux, root.path(), "data/elastos"), &refused);
    let notice = service.ensure_linger().unwrap();
    assert!(notice.contains("while you are logged in") && notice.contains("enable-linger"));
}
