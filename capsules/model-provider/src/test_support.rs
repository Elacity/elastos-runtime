use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

static TEST_ROOT_COUNTER: AtomicU64 = AtomicU64::new(1);
const TEST_ROOT_PREFIX: &str = "model-provider-";

thread_local! {
    static TEST_ROOT_REGISTRY: TestRootRegistry = TestRootRegistry::default();
}

#[derive(Default)]
struct TestRootRegistry {
    roots: RefCell<Vec<PathBuf>>,
}

impl Drop for TestRootRegistry {
    fn drop(&mut self) {
        let roots = self.roots.get_mut();
        while let Some(root) = roots.pop() {
            if !is_registered_model_provider_root(&root) {
                panic!(
                    "invalid model-provider test root registered for cleanup: {}",
                    root.display()
                );
            }
            match std::fs::remove_dir_all(&root) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => panic!(
                    "failed to remove model-provider test root {}: {err}",
                    root.display()
                ),
            }
        }
    }
}

pub(crate) fn temp_root_path(prefix: &str, label: &str) -> PathBuf {
    assert!(
        prefix.starts_with(TEST_ROOT_PREFIX),
        "model-provider test root prefix must start with {TEST_ROOT_PREFIX}"
    );
    let root = std::env::temp_dir().join(format!(
        "{prefix}-{label}-{}-{}",
        std::process::id(),
        TEST_ROOT_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    assert!(
        is_registered_model_provider_root(&root),
        "model-provider test root must remain under temp_dir with {TEST_ROOT_PREFIX} prefix: {}",
        root.display()
    );
    TEST_ROOT_REGISTRY.with(|registry| {
        registry.roots.borrow_mut().push(root.clone());
    });
    root
}

#[cfg(unix)]
pub(crate) fn sha256_file(path: &Path) -> String {
    use sha2::Digest as _;

    let mut file = std::fs::File::open(path).unwrap();
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer).unwrap();
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

#[cfg(unix)]
pub(crate) fn write_fake_llama_server(root: &Path, mode: &str) -> (PathBuf, PathBuf, PathBuf) {
    let engine = root.join("bin/fake-llama-server");
    let model = root.join("models/fake.gguf");
    let events = root.join("models/fake.events");
    std::fs::create_dir_all(engine.parent().unwrap()).unwrap();
    std::fs::create_dir_all(model.parent().unwrap()).unwrap();
    std::fs::write(
        &engine,
        br#"#!/usr/bin/python3
import http.server
import json
import os
import pathlib
import signal
import subprocess
import sys
import threading
import time

def arg(name):
    return sys.argv[sys.argv.index(name) + 1]

model_path = pathlib.Path(arg('-m'))
port = int(arg('--port'))
alias = arg('--alias')
mode = model_path.read_text(encoding='utf-8').strip()
events = model_path.with_suffix('.events')
unresponsive = model_path.with_suffix('.unresponsive')
wrong_alias = model_path.with_suffix('.wrong-alias')

if mode == 'mutable_readiness':
    unresponsive.unlink(missing_ok=True)
    wrong_alias.unlink(missing_ok=True)

def record(value):
    with events.open('a', encoding='utf-8') as stream:
        stream.write(value + '\n')

def terminate(_signum, _frame):
    record('term')
    raise SystemExit(0)

def ignore_termination(_signum, _frame):
    record('term_ignored')

if mode in ('ignore_term', 'timeout_ignore_term'):
    signal.signal(signal.SIGTERM, ignore_termination)
else:
    signal.signal(signal.SIGTERM, terminate)
record('start:' + str(os.getpid()))
record('parent:' + str(os.getppid()))
if mode == 'healthy_with_subtree':
    subtree = subprocess.Popen([
        sys.executable,
        '-c',
        'import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(3600)',
    ])
    record('subtree:' + str(subtree.pid))
if mode in ('timeout', 'timeout_ignore_term'):
    while True:
        time.sleep(1)

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, _format, *_args):
        pass

    def do_GET(self):
        if self.path == '/health':
            body = {'status': 'ok'}
        elif self.path == '/v1/models':
            if mode == 'slow_health':
                time.sleep(0.225)
            if unresponsive.exists():
                threading.Event().wait()
            model_id = 'wrong-model' if wrong_alias.exists() else alias
            body = {'object': 'list', 'data': [{'id': model_id, 'object': 'model'}]}
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(json.dumps(body).encode('utf-8'))

    def do_POST(self):
        if self.path != '/v1/chat/completions':
            self.send_error(404)
            return
        length = int(self.headers.get('Content-Length', '0'))
        body = json.loads(self.rfile.read(length))
        prompt = body['messages'][0]['content']
        if body.get('model') != alias or body.get('chat_template_kwargs') != {'enable_thinking': False}:
            self.send_error(400)
            return
        record('request:' + prompt)
        if mode == 'crash_once' and prompt == 'crash':
            marker = model_path.with_suffix('.crashed')
            if not marker.exists():
                marker.write_text('crashed', encoding='utf-8')
                os._exit(17)
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.end_headers()
        if prompt == 'stall':
            while True:
                try:
                    self.wfile.write(b': keepalive\n\n')
                    self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError):
                    record('cancelled')
                    return
                time.sleep(0.025)
        payload = json.dumps({'choices': [{'delta': {'content': 'reply:' + prompt}}]})
        self.wfile.write(('data: ' + payload + '\n\ndata: [DONE]\n\n').encode('utf-8'))
        self.wfile.flush()

server = http.server.ThreadingHTTPServer(('127.0.0.1', port), Handler)
server.daemon_threads = True
server.serve_forever()
"#,
    )
    .unwrap();
    std::fs::write(&model, mode.as_bytes()).unwrap();
    std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&model, std::fs::Permissions::from_mode(0o600)).unwrap();
    (engine, model, events)
}

fn is_registered_model_provider_root(path: &Path) -> bool {
    let temp_dir = std::env::temp_dir();
    let Ok(relative) = path.strip_prefix(&temp_dir) else {
        return false;
    };
    if relative.components().count() != 1 {
        return false;
    }
    relative
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with(TEST_ROOT_PREFIX))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::temp_root_path;
    use std::thread;

    #[test]
    fn registered_test_root_is_removed_when_thread_exits() {
        let root = thread::spawn(|| {
            let root = temp_root_path("model-provider-cleanup", "thread-exit");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("marker.txt"), b"marker").unwrap();
            root
        })
        .join()
        .unwrap();

        assert!(
            !root.exists(),
            "registered model-provider test root must be removed after thread exit: {}",
            root.display()
        );
    }
}
