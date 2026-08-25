//! AI-facing structured sink: one JSON object per line, one bounded ring file per component.
//!
//! Observe-only by construction: it renders the same already-formatted (and already `fp()`'d)
//! record fields the line sinks see — no extra data is captured. Enable it by adding the sink at
//! init; the `elastos` binary wires it to `ELASTOS_LOG_JSON_DIR`.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::{LogRecord, LogSink};

/// Rotate threshold per component file (`<component>.jsonl` → `<component>.jsonl.1`), keeping at
/// most two generations (~2× this many bytes) per component.
const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024;

struct ComponentFile {
    file: fs::File,
    bytes: u64,
}

/// Per-component JSON-lines ring sink. Each record appends one JSON object
/// (`ts`, `level`, `component`, `target`, `msg`) to `<dir>/<component>.jsonl`; when the file would
/// exceed the byte cap it is renamed to `.jsonl.1` (replacing the previous generation) and a fresh
/// file is started. Cheap and non-panicking per the [`LogSink`] contract: any IO failure drops the
/// record.
pub struct JsonRingSink {
    dir: PathBuf,
    max_bytes: u64,
    files: Mutex<HashMap<String, ComponentFile>>,
}

impl JsonRingSink {
    /// Create the sink rooted at `dir` (created if absent) with the default 5MB rotate threshold.
    pub fn new(dir: impl AsRef<Path>) -> std::io::Result<JsonRingSink> {
        JsonRingSink::with_max_bytes(dir, DEFAULT_MAX_BYTES)
    }

    /// Like [`JsonRingSink::new`] with an explicit rotate threshold (tests use small values).
    pub fn with_max_bytes(dir: impl AsRef<Path>, max_bytes: u64) -> std::io::Result<JsonRingSink> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        Ok(JsonRingSink {
            dir,
            max_bytes: max_bytes.max(1),
            files: Mutex::new(HashMap::new()),
        })
    }

    /// Component names become file names: keep `[A-Za-z0-9._-]`, map anything else to `_`, and
    /// never allow an empty or dot-leading stem (no path traversal, no hidden files).
    fn file_stem(component: &str) -> String {
        let mut stem: String = component
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        while stem.starts_with('.') {
            stem.remove(0);
        }
        if stem.is_empty() {
            stem.push_str("unknown");
        }
        stem
    }

    fn path_for(&self, component: &str) -> PathBuf {
        self.dir
            .join(format!("{}.jsonl", JsonRingSink::file_stem(component)))
    }

    fn open_append(path: &Path) -> std::io::Result<ComponentFile> {
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(ComponentFile { file, bytes })
    }
}

impl LogSink for JsonRingSink {
    fn write(&self, rec: &LogRecord) {
        let line = serde_json::json!({
            "ts": rec.ts,
            "level": rec.level.as_str(),
            "component": rec.component,
            "target": rec.target,
            "msg": rec.message,
        })
        .to_string();

        let Ok(mut files) = self.files.lock() else {
            return;
        };
        let key = JsonRingSink::file_stem(rec.component);
        let path = self.path_for(rec.component);

        if !files.contains_key(&key) {
            match JsonRingSink::open_append(&path) {
                Ok(cf) => {
                    files.insert(key.clone(), cf);
                }
                Err(_) => return,
            }
        }
        let Some(cf) = files.get_mut(&key) else {
            return;
        };

        let needed = line.len() as u64 + 1;
        if cf.bytes > 0 && cf.bytes + needed > self.max_bytes {
            // Rotate: current generation becomes `.1` (replacing the previous one), fresh file.
            let rotated = self.dir.join(format!("{}.jsonl.1", key));
            let _ = fs::rename(&path, &rotated);
            match JsonRingSink::open_append(&path) {
                Ok(new_cf) => *cf = new_cf,
                Err(_) => {
                    files.remove(&key);
                    return;
                }
            }
        }

        if writeln!(cf.file, "{}", line).is_ok() {
            cf.bytes += needed;
        }
    }

    fn flush(&self) {
        if let Ok(mut files) = self.files.lock() {
            for cf in files.values_mut() {
                let _ = cf.file.flush();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::JsonRingSink;
    use crate::{now_rfc3339, Level, LogRecord, LogSink};

    fn rec<'a>(component: &'a str, message: &'a str) -> LogRecord<'a> {
        LogRecord {
            level: Level::Warn,
            component,
            target: "elastos_server::api",
            message,
            ts: now_rfc3339(),
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("elog-json-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn writes_one_parseable_json_object_per_line() {
        let dir = temp_dir("shape");
        let sink = JsonRingSink::new(&dir).unwrap();
        sink.write(&rec("gateway.auth", "token rejected fp:a1b2c3d4"));
        sink.write(&rec("gateway.auth", "second"));
        sink.flush();

        let body = std::fs::read_to_string(dir.join("gateway.auth.jsonl")).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["level"], "WARN");
        assert_eq!(v["component"], "gateway.auth");
        assert_eq!(v["target"], "elastos_server::api");
        assert_eq!(v["msg"], "token rejected fp:a1b2c3d4");
        assert!(v["ts"].as_str().unwrap().ends_with('Z'));
    }

    #[test]
    fn each_component_gets_its_own_file() {
        let dir = temp_dir("split");
        let sink = JsonRingSink::new(&dir).unwrap();
        sink.write(&rec("carrier", "a"));
        sink.write(&rec("vm.vz", "b"));
        sink.flush();
        assert!(dir.join("carrier.jsonl").exists());
        assert!(dir.join("vm.vz.jsonl").exists());
    }

    #[test]
    fn rotates_to_dot_one_at_the_byte_cap_and_bounds_total_size() {
        let dir = temp_dir("rotate");
        let sink = JsonRingSink::with_max_bytes(&dir, 400).unwrap();
        for i in 0..60 {
            sink.write(&rec("cmd.serve", &format!("event number {}", i)));
        }
        sink.flush();

        let current = dir.join("cmd.serve.jsonl");
        let rotated = dir.join("cmd.serve.jsonl.1");
        assert!(rotated.exists(), "rotation must have happened");
        let cur_len = std::fs::metadata(&current).unwrap().len();
        let rot_len = std::fs::metadata(&rotated).unwrap().len();
        assert!(cur_len <= 400, "current stays under the cap, got {cur_len}");
        assert!(
            rot_len <= 400,
            "rotated generation under the cap, got {rot_len}"
        );
        // Newest record is in the current file; oldest surviving in .1; nothing else kept.
        let body = std::fs::read_to_string(&current).unwrap();
        assert!(body.contains("event number 59"));
    }

    #[test]
    fn hostile_component_names_cannot_escape_the_directory() {
        let dir = temp_dir("sanitize");
        let sink = JsonRingSink::new(&dir).unwrap();
        sink.write(&rec("../evil", "x"));
        sink.flush();
        // '/' maps to '_', leading dots are stripped: "../evil" → "_evil.jsonl" inside dir.
        assert!(dir.join("_evil.jsonl").exists());
        assert!(!dir.parent().unwrap().join("evil.jsonl").exists());
    }
}
