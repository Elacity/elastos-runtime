use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
