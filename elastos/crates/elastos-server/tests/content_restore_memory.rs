//! Startup restore and repair walk the data directory for complete local
//! objects and import them into the local Kubo. Runtime's heap must stay
//! bounded however large those objects are: a Qwen-sized admitted model once
//! grew Home past 7 GB and the kernel OOM-killed an 8 GB host. This binary
//! owns its global allocator to measure that.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeSet;
use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;

use elastos_runtime::provider::{
    Provider, ProviderError, ProviderRegistry, ResourceRequest, ResourceResponse,
};
use elastos_server::content::ContentProvider;
use serde_json::{json, Value};
use sha2::Digest;
use tokio::sync::Mutex;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Counts live heap bytes and refuses growth past `LIMIT`, so a regression
/// fails this test instead of exhausting the host running it.
struct Counting;

fn reserve(bytes: usize) -> bool {
    let live = LIVE.fetch_add(bytes, Relaxed) + bytes;
    if live > LIMIT.load(Relaxed) {
        LIVE.fetch_sub(bytes, Relaxed);
        return false;
    }
    PEAK.fetch_max(live, Relaxed);
    true
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !reserve(layout.size()) {
            return std::ptr::null_mut();
        }
        let pointer = System.alloc(layout);
        if pointer.is_null() {
            LIVE.fetch_sub(layout.size(), Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout);
        LIVE.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let old = layout.size();
        if size > old && !reserve(size - old) {
            return std::ptr::null_mut();
        }
        let moved = System.realloc(pointer, layout, size);
        if moved.is_null() {
            if size > old {
                LIVE.fetch_sub(size - old, Relaxed);
            }
        } else if size < old {
            LIVE.fetch_sub(old - size, Relaxed);
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Restore and repair may hold manifests and paths; never object content.
const HEAP_BOUND: usize = 16 * 1024 * 1024;
const HELD_CID: &str = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";
const MEDIA_CID: &str = "bafybeihgnsjhpoktqbyspaqv6moblyny3txs5nkjdxfx7wm346odxkhlrm";
// Sparse fixture files read as zeros; these are SHA-256 digests of N zero bytes.
const QWEN_WEIGHTS_BYTES: u64 = 1_117_320_736;
const QWEN_ZEROS_SHA256: &str = "8de2df505f2c46dd3b893364749702df7863e3183de4f386e868373848a157e9";
const MEDIA_BYTES: u64 = 200 * 1024 * 1024;
const MEDIA_ZEROS_SHA256: &str = "72abf2ca8f36943ebe2e49ca3a51d409ca5f0bfcffab6c9d25643c17c32889da";

enum Body<'a> {
    Bytes(&'a [u8]),
    Zeros(u64, &'a str),
}

type Descriptor = Vec<(String, u64)>;

/// Writes a complete local object (every file matches its manifest entry) and
/// returns the paths and sizes a streamed import must name, manifest last.
fn write_object(dir: &Path, files: &[(&str, Body<'_>)]) -> Descriptor {
    std::fs::create_dir_all(dir).unwrap();
    let mut entries = Vec::new();
    let mut descriptor = Vec::new();
    for (path, body) in files {
        let (size, sha256) = match body {
            Body::Bytes(bytes) => {
                std::fs::write(dir.join(path), bytes).unwrap();
                (
                    bytes.len() as u64,
                    format!("{:x}", sha2::Sha256::digest(bytes)),
                )
            }
            Body::Zeros(size, sha256) => {
                let file = std::fs::File::create(dir.join(path)).unwrap();
                file.set_len(*size).unwrap();
                (*size, sha256.to_string())
            }
        };
        entries.push(json!({"path": path, "sha256": sha256, "size": size}));
        descriptor.push((path.to_string(), size));
    }
    let manifest = serde_json::to_vec(&json!({
        "schema": "elastos.content.object.manifest/v1",
        "kind": "directory",
        "content_digest": format!("sha256:{}", "0".repeat(64)),
        "files": entries,
    }))
    .unwrap();
    std::fs::write(dir.join("_elastos_object.json"), &manifest).unwrap();
    descriptor.push(("_elastos_object.json".into(), manifest.len() as u64));
    descriptor
}

/// Local Kubo stand-in. A streamed import names a directory and its file
/// sizes, never bytes, and stores the object unpinned under its own root CID;
/// a pin succeeds only for a stored CID.
#[derive(Default)]
struct Ipfs {
    imported: Mutex<Vec<(String, Descriptor)>>,
    stored: Mutex<BTreeSet<String>>,
    pinned: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Provider for Ipfs {
    async fn handle(&self, _: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider("raw operations only".into()))
    }

    fn schemes(&self) -> Vec<&'static str> {
        Vec::new()
    }

    fn name(&self) -> &'static str {
        "restore-memory-ipfs"
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        match request["op"].as_str().unwrap() {
            "runtime_add_local_object" => {
                let root = Path::new(request["object"]["root"].as_str().unwrap());
                let name = root.file_name().unwrap().to_str().unwrap().to_owned();
                let files = request["object"]["files"].as_array().unwrap().iter();
                let files = files
                    .map(|f| {
                        (
                            f["path"].as_str().unwrap().into(),
                            f["size"].as_u64().unwrap(),
                        )
                    })
                    .collect();
                let cid = if name == "media-object" {
                    MEDIA_CID
                } else {
                    HELD_CID
                };
                self.imported.lock().await.push((name, files));
                self.stored.lock().await.insert(cid.to_owned());
                Ok(json!({"status": "ok", "data": {"cid": cid}}))
            }
            "pin" => {
                let cid = request["cid"].as_str().unwrap().to_owned();
                if !self.stored.lock().await.contains(&cid) {
                    return Ok(
                        json!({"status": "error", "code": "pin_failed", "message": "not held"}),
                    );
                }
                self.pinned.lock().await.push(cid);
                Ok(json!({"status": "ok", "data": {}}))
            }
            _ => Ok(json!({"status": "ok", "data": {}})),
        }
    }
}

async fn heap_growth<T>(work: impl Future<Output = T>) -> (T, usize) {
    let baseline = LIVE.load(Relaxed);
    PEAK.store(baseline, Relaxed);
    LIMIT.store(baseline + 16 * HEAP_BOUND, Relaxed);
    let output = work.await;
    LIMIT.store(usize::MAX, Relaxed);
    (output, PEAK.load(Relaxed) - baseline)
}

#[tokio::test]
async fn restore_and_repair_stream_large_objects_and_skip_model_packages() {
    let data = tempfile::tempdir().unwrap();
    let held = write_object(
        &data.path().join("held-object"),
        &[("index.md", Body::Bytes(b"hello object"))],
    );
    // Larger than the old 64 MiB JSON import; must still restore and repair.
    let media = write_object(
        &data.path().join("media-object"),
        &[("media.bin", Body::Zeros(MEDIA_BYTES, MEDIA_ZEROS_SHA256))],
    );
    // Runtime-private admitted model packages, laid out as preparation admits
    // them. Neither the Qwen-sized nor a small one is a Content replica.
    let models = data.path().join("model-preparation");
    write_object(
        &models.join(format!("admitted-{}", "a".repeat(64))),
        &[
            ("capsule.json", Body::Bytes(br#"{"name":"qwen"}"#)),
            (
                "weights.gguf",
                Body::Zeros(QWEN_WEIGHTS_BYTES, QWEN_ZEROS_SHA256),
            ),
        ],
    );
    write_object(
        &models.join(format!("admitted-{}", "b".repeat(64))),
        &[
            ("capsule.json", Body::Bytes(br#"{"name":"tiny"}"#)),
            ("weights.gguf", Body::Bytes(b"GGUF\x03\0\0\0")),
        ],
    );
    let registry = Arc::new(ProviderRegistry::new());
    let ipfs = Arc::new(Ipfs::default());
    registry
        .register_sub_provider("ipfs", ipfs.clone())
        .await
        .unwrap();
    let content = Arc::new(ContentProvider::new(
        data.path().to_path_buf(),
        Arc::downgrade(&registry),
    ));

    // Startup restore imports both ordinary objects by path and size.
    let ((), growth) = heap_growth(content.restore_complete_local_objects()).await;
    assert!(growth <= HEAP_BOUND, "restore heap grew {growth} bytes");
    let mut imported = std::mem::take(&mut *ipfs.imported.lock().await);
    imported.sort();
    assert_eq!(
        imported,
        [
            ("held-object".to_owned(), held),
            ("media-object".to_owned(), media.clone())
        ]
    );
    // No Content receipt names these CIDs, so restore pins neither.
    assert!(ipfs.pinned.lock().await.is_empty());

    // Repair after the local Kubo lost the object: the pin fails, so Content
    // imports complete objects unpinned until one has the requested CID, and
    // pins only that one.
    ipfs.stored.lock().await.clear();
    let (response, growth) =
        heap_growth(content.send_raw(&json!({"op": "ensure", "cid": MEDIA_CID}))).await;
    assert!(growth <= HEAP_BOUND, "repair heap grew {growth} bytes");
    let response = response.unwrap();
    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(
        response["data"]["availability"]["policy"],
        "local_object_import"
    );
    assert_eq!(response["data"]["availability"]["replicas"], 1);
    assert!(ipfs
        .imported
        .lock()
        .await
        .contains(&("media-object".to_owned(), media)));
    assert_eq!(*ipfs.pinned.lock().await, [MEDIA_CID]);
}
