// Opt-in proofs only. The caller supplies read-only operator artifacts; the
// existing disposable process proof owns every import, journal and child.
//
// Model bytes are platform-independent, so one pinned model runs on every
// proved host profile. The engine executable has its own identity per
// platform: the same llama.cpp release ships a different bundle per host.
struct PinnedModel {
    /// `ELASTOS_TEST_<prefix>_PATH`, `_LICENSE_PATH` and `_ENGINE_DATA` name
    /// the caller-owned read-only inputs.
    env_prefix: &'static str,
    capsule_name: &'static str,
    bytes: u64,
    sha: &'static str,
    license_bytes: u64,
    license_sha: &'static str,
    quantization: &'static str,
    minimum_memory_mb: u64,
    base_repository: &'static str,
    base_revision: &'static str,
    quantized_repository: &'static str,
    quantized_revision: &'static str,
    provenance: &'static str,
    /// Whole-proof budget: import, loopback transfer, admission, reply/restart.
    deadline_secs: u64,
    /// Accepts the completed text of the first reply.
    reply_accepts: fn(&str) -> bool,
    /// Keeps this model generating long enough for an active cancellation
    /// to be observed while text deltas stream.
    long_prompt: &'static str,
}

const QWEN: PinnedModel = PinnedModel {
    env_prefix: "QWEN",
    capsule_name: "qwen-isolated-proof",
    bytes: 6_169_341_984,
    sha: "d784ce9eda1a5a7b51e8f705a9e6310844bf4f173654d115823c775fdea56d43",
    license_bytes: 11544,
    license_sha: "bbedc3fda3305820b977265f01b8619d87570a6739de3a5582c3464840f1e57a",
    quantization: "Q4_K_M",
    minimum_memory_mb: 8192,
    base_repository: "Qwen/Qwen3.5-9B",
    base_revision: "c202236235762e1c871ad0ccb60c8ee5ba337b9a",
    quantized_repository: "bartowski/Qwen_Qwen3.5-9B-GGUF",
    quantized_revision: "bc658f7a1ae222ee901a565aafd2f5b8c839e176",
    provenance: "Isolated operator/test publisher attestation, not an upstream signature. Weights match the pinned bartowski quantization. Its README names Qwen/Qwen3.5-9B and llama.cpp b9222 but does not attest a base conversion revision or include a standalone license. The base_revision is an independently checked upstream reference with Apache-2.0 license, not an asserted conversion input. LICENSE and LICENSE.base reproduce that checked license for this isolated proof.\n",
    deadline_secs: 3500,
    // A 9B instruct model follows the one-word instruction exactly.
    reply_accepts: |text| {
        text.trim()
            .trim_end_matches(['.', '!'])
            .eq_ignore_ascii_case("ready")
    },
    long_prompt: "Write a continuous numbered list of detailed, distinct sentences about ordinary garden plants. Produce at least 12000 characters of text. Continue the list without a summary or closing remarks.",
};

const SMOLLM2: PinnedModel = PinnedModel {
    env_prefix: "SMOLLM2",
    capsule_name: "smollm2-isolated-proof",
    bytes: 144_811_072,
    sha: "c4a3dd037301b6ecea31d6da37f5cd793ead920dd5ddfe6d589294628d6ce66a",
    // Canonical Apache License 2.0 text (apache.org/licenses/LICENSE-2.0.txt).
    license_bytes: 11358,
    license_sha: "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30",
    quantization: "Q8_0",
    minimum_memory_mb: 1024,
    base_repository: "HuggingFaceTB/SmolLM2-135M-Instruct",
    base_revision: "12fd25f77366fa6b3b4b768ec3050bf629380bac",
    quantized_repository: "unsloth/SmolLM2-135M-Instruct-GGUF",
    quantized_revision: "9e6855bc4be717fca1ef21360a1db4b29d5c559a",
    provenance: "Isolated operator/test publisher attestation, not an upstream signature. Weights match the pinned unsloth Q8_0 quantization, whose README names HuggingFaceTB/SmolLM2-135M-Instruct as base_model under license apache-2.0. Neither repository ships a standalone LICENSE file; LICENSE and LICENSE.base reproduce the canonical Apache License 2.0 text for this isolated proof. The base_revision is the independently checked upstream head, not an asserted conversion input.\n",
    deadline_secs: 900,
    // A 135M model answers freely under the provider's default sampling; the
    // proof binds a completed, non-empty streamed reply, not its wording.
    reply_accepts: |text| !text.trim().is_empty(),
    // A small model ignores length instructions but keeps narrating a story
    // for well over a thousand tokens.
    long_prompt: "Write a very long story about a lighthouse keeper. Keep writing new chapters and never stop.",
};

/// SHA-256 of the verified `llama-server` executable inside the pinned
/// b10516 bundle for each proved host profile.
fn pinned_engine_sha(platform: &str) -> &'static str {
    match platform {
        "darwin-arm64" => "d0878274b8d6bd3c8ea26a78eb66cd1ffd943d007c62b9dff31c8aa99922d713",
        "linux-amd64" => "fa24fc90877d1edc68990af5f4f8e476256959357d7f58cf59910e5657f7403f",
        other => panic!("no pinned b10516 engine identity for {other}"),
    }
}

const PACKAGE_ADD_ARGS: &[&str] = &[
    "add",
    "--recursive=true",
    "--quieter=true",
    "--wrap-with-directory=true",
    "--only-hash=false",
    "--pin=true",
    "--cid-version=1",
    "--hash=sha2-256",
    "--raw-leaves=true",
    "--chunker=size-262144",
    "--trickle=false",
    "--max-file-links=174",
    "--max-directory-links=0",
    "--max-hamt-fanout=256",
    "--inline=false",
    "--nocopy=false",
    "--fscache=false",
    "--preserve-mode=false",
    "--preserve-mtime=false",
    "--empty-dirs=false",
    "--progress=false",
    "--fast-provide-root=false",
    "--fast-provide-wait=false",
];

fn proof_import_profile() -> serde_json::Value {
    serde_json::json!({
        "CidVersion":1,"UnixFSRawLeaves":true,"UnixFSChunker":"size-262144", "HashFunction":"sha2-256",
        "UnixFSFileMaxLinks":174,"UnixFSDirectoryMaxLinks":0,"UnixFSHAMTDirectoryMaxFanout":256,
        "UnixFSHAMTDirectorySizeThreshold":"256KiB","UnixFSHAMTDirectorySizeEstimation":"links",
        "UnixFSDAGLayout":"balanced","FastProvideRoot":false,"FastProvideWait":false
    })
}

async fn import_package_parts(
    kubo: &Path,
    root: &Path,
    repo: &Path,
    seed: &Path,
    metadata_args: &[&str],
    weights: &File,
    deadline: Instant,
) -> (String, String) {
    let mut args = PACKAGE_ADD_ARGS.to_vec();
    args.retain(|arg| *arg != "--wrap-with-directory=true");
    args.push("--stdin-name=weights.gguf");
    let mut import = command(kubo, root, repo, seed, &args);
    let mut input = weights.try_clone().unwrap();
    input.rewind().unwrap();
    import.stdin(input);
    eprintln!("isolated bootstrap: streamed weights import");
    let weights_cid = run(import, root, deadline).await;
    eprintln!("isolated bootstrap: metadata directory import");
    let metadata_cid = run(
        command(kubo, root, repo, seed, metadata_args),
        root,
        deadline,
    )
    .await;
    (metadata_cid, weights_cid)
}

async fn import_streamed_package(
    kubo: &Path,
    root: &Path,
    repo: &Path,
    seed: &Path,
    metadata_args: &[&str],
    weights: &File,
    deadline: Instant,
) -> String {
    let (metadata_cid, weights_cid) =
        import_package_parts(kubo, root, repo, seed, metadata_args, weights, deadline).await;
    // Kubo 0.40.1 object patch checks ProtoNode.Size (including references)
    // against its raw-block limit. Use the documented MFS composition instead:
    // https://github.com/ipfs/kubo/blob/v0.40.1/core/commands/object/patch.go
    // This fixed MFS name exists only in the fresh disposable publisher repo.
    eprintln!("isolated bootstrap: MFS directory composition");
    run(
        command(
            kubo,
            root,
            repo,
            seed,
            &[
                "files",
                "cp",
                &format!("/ipfs/{metadata_cid}"),
                "/proof-package",
            ],
        ),
        root,
        deadline,
    )
    .await;
    run(
        command(
            kubo,
            root,
            repo,
            seed,
            &[
                "files",
                "cp",
                &format!("/ipfs/{weights_cid}"),
                "/proof-package/weights.gguf",
            ],
        ),
        root,
        deadline,
    )
    .await;
    let root_cid = run(
        command(
            kubo,
            root,
            repo,
            seed,
            &["files", "stat", "--hash", "/proof-package"],
        ),
        root,
        deadline,
    )
    .await;
    // Preserve the exact DAG codec/hash in the canonical CIDv1 representation.
    let cid: cid::Cid = root_cid.parse().unwrap();
    assert_eq!(cid.codec(), 0x70);
    cid::Cid::new_v1(cid.codec(), *cid.hash()).to_string()
}

#[tokio::test]
#[ignore = "requires explicit pinned ELASTOS_TEST_KUBO_PATH; run before the Qwen import"]
async fn model_preparation_streamed_bootstrap_matches_directory_cid() {
    let kubo =
        PathBuf::from(std::env::var_os("ELASTOS_TEST_KUBO_PATH").expect("explicit pinned Kubo"));
    assert!(kubo.is_absolute());
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    let (capacity, free) = volume_bytes(&File::open(&root_path).unwrap());
    storage::require_space_floor(capacity, free, 64 * 1024 * 1024).unwrap();
    let repo = root_path.join("repo");
    let seed = root_path.join("seed");
    fs::create_dir(&seed).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    assert_eq!(
        run(
            command(&kubo, &root_path, &repo, &seed, &["version", "--number"]),
            &root_path,
            deadline
        )
        .await,
        "0.40.1"
    );
    run(
        command(&kubo, &root_path, &repo, &seed, &["init", "--empty-repo"]),
        &root_path,
        deadline,
    )
    .await;
    let config_path = repo.join("config");
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    config["Import"] = proof_import_profile();
    fs::write(config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    fs::write(seed.join("capsule.json"), b"bounded bootstrap fixture").unwrap();
    // Cross both the chunk boundary and the old object-patch cumulative-size
    // rejection at 2 MiB. The bounded fixture remains below 3 MiB.
    let bytes = vec![0x6d; 2 * 1024 * 1024 + 17];
    let source = root_path.join("source");
    fs::write(&source, &bytes).unwrap();
    let weights = File::open(&source).unwrap();
    let mut args = PACKAGE_ADD_ARGS.to_vec();
    args.push("capsule.json");
    let (metadata_cid, weights_cid) =
        import_package_parts(&kubo, &root_path, &repo, &seed, &args, &weights, deadline).await;
    // Measure the old command failure before exercising the replacement.
    // The same valid chunked DAG exceeds 2 MiB only cumulatively.
    let mut diagnostic = tempfile::tempfile_in(&root_path).unwrap();
    let mut legacy = OwnedChild(
        command(
            &kubo,
            &root_path,
            &repo,
            &seed,
            &[
                "object",
                "patch",
                "add-link",
                &metadata_cid,
                "weights.gguf",
                &weights_cid,
            ],
        )
        .stdout(Stdio::null())
        .stderr(diagnostic.try_clone().unwrap())
        .spawn()
        .unwrap(),
    );
    let legacy_status = loop {
        assert!(Instant::now() < deadline);
        assert!(diagnostic.metadata().unwrap().len() <= 4096);
        if let Some(status) = legacy.0.try_wait().unwrap() {
            break status;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    drop(legacy);
    diagnostic.rewind().unwrap();
    let mut message = String::new();
    diagnostic.take(4097).read_to_string(&mut message).unwrap();
    assert_eq!(legacy_status.code(), Some(1));
    assert!(
        message.len() <= 4096 && message.contains("produced block is over 2MiB"),
        "unexpected legacy command rejection: {message}"
    );
    let streamed =
        import_streamed_package(&kubo, &root_path, &repo, &seed, &args, &weights, deadline).await;
    let block: serde_json::Value = serde_json::from_str(
        &run(
            command(
                &kubo,
                &root_path,
                &repo,
                &seed,
                &["block", "stat", "--enc=json", &streamed],
            ),
            &root_path,
            deadline,
        )
        .await,
    )
    .unwrap();
    assert!(
        block["Size"].as_u64().unwrap() < 2 * 1024 * 1024,
        "directory root must remain a normal bounded block"
    );
    fs::write(seed.join("weights.gguf"), &bytes).unwrap();
    args.push("weights.gguf");
    args.retain(|arg| *arg != "--only-hash=false");
    args.push("--only-hash=true");
    let canonical = run(
        command(&kubo, &root_path, &repo, &seed, &args),
        &root_path,
        deadline,
    )
    .await;
    drop(weights);
    root.close().unwrap();
    assert!(!root_path.exists());
    assert!(canonical_cid(&streamed, 0x70));
    assert_eq!(
        streamed, canonical,
        "stream/MFS composition must match the canonical full directory hash"
    );
}

struct PinnedModelProof {
    model: &'static PinnedModel,
    weights: File,
    license: Vec<u8>,
    engine_data: PathBuf,
    provider: PathBuf,
}

fn proof_file(path: &Path, max_bytes: u64) -> File {
    use std::os::unix::fs::OpenOptionsExt as _;
    assert!(path.is_absolute());
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .unwrap();
    let meta = file.metadata().unwrap();
    assert!(meta.is_file() && meta.len() <= max_bytes && meta.nlink() == 1);
    assert_eq!(meta.uid(), unsafe { libc::geteuid() });
    assert_eq!(meta.mode() & 0o022, 0);
    file
}

fn streamed_sha(file: &mut File) -> String {
    let before = file.metadata().unwrap();
    file.rewind().unwrap();
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let after = file.metadata().unwrap();
    assert_eq!(
        (
            before.dev(),
            before.ino(),
            before.len(),
            before.mtime(),
            before.mtime_nsec(),
            before.ctime(),
            before.ctime_nsec()
        ),
        (
            after.dev(),
            after.ino(),
            after.len(),
            after.mtime(),
            after.mtime_nsec(),
            after.ctime(),
            after.ctime_nsec()
        )
    );
    file.rewind().unwrap();
    hex::encode(digest.finalize())
}

impl PinnedModel {
    fn assert_weights(&self, path: &Path) {
        let mut file = proof_file(path, self.bytes);
        assert_eq!(file.metadata().unwrap().len(), self.bytes);
        assert_eq!(streamed_sha(&mut file), self.sha);
    }

    fn env_path(&self, suffix: &str) -> PathBuf {
        let name = format!("ELASTOS_TEST_{}_{suffix}", self.env_prefix);
        PathBuf::from(
            std::env::var_os(&name)
                .unwrap_or_else(|| panic!("explicit {name} proof prerequisite")),
        )
    }
}

impl PinnedModelProof {
    fn from_env(model: &'static PinnedModel) -> Self {
        let mut weights = proof_file(&model.env_path("PATH"), model.bytes);
        assert_eq!(weights.metadata().unwrap().len(), model.bytes);
        assert_eq!(streamed_sha(&mut weights), model.sha);
        let mut license = Vec::new();
        proof_file(&model.env_path("LICENSE_PATH"), model.license_bytes)
            .read_to_end(&mut license)
            .unwrap();
        assert_eq!(license.len() as u64, model.license_bytes);
        assert_eq!(hex::encode(Sha256::digest(&license)), model.license_sha);
        Self {
            model,
            weights,
            license,
            engine_data: model.env_path("ENGINE_DATA"),
            provider: PathBuf::from(
                std::env::var_os("ELASTOS_TEST_MODEL_PROVIDER_PATH")
                    .expect("explicit ELASTOS_TEST_MODEL_PROVIDER_PATH proof prerequisite"),
            ),
        }
    }

    fn package(
        &self,
    ) -> (
        serde_json::Value,
        std::collections::BTreeMap<String, Vec<u8>>,
    ) {
        let model = self.model;
        let mut payload = super::super::super::tests::model_catalog_fixture();
        let capsule = &mut payload["entries"][0]["capsule_manifest"];
        capsule["name"] = serde_json::json!(model.capsule_name);
        capsule["model_content"]["quantization"] = serde_json::json!(model.quantization);
        capsule["model_content"]["minimum_memory_mb"] = serde_json::json!(model.minimum_memory_mb);
        let provenance = &mut capsule["model_content"]["provenance"];
        provenance["base_repository"] = serde_json::json!(model.base_repository);
        provenance["base_revision"] = serde_json::json!(model.base_revision);
        provenance["quantized_repository"] = serde_json::json!(model.quantized_repository);
        provenance["quantized_revision"] = serde_json::json!(model.quantized_revision);
        let mut files = std::collections::BTreeMap::from([
            ("LICENSE".into(), self.license.clone()), ("LICENSE.base".into(), self.license.clone()),
            ("PROVENANCE.md".into(), model.provenance.as_bytes().to_vec()),
            ("capsule.json".into(), serde_json::to_vec(capsule).unwrap()),
        ]);
        let object = &mut payload["entries"][0]["object_manifest"];
        let mut digest = Sha256::new();
        for file in object["files"].as_array_mut().unwrap() {
            let path = file["path"].as_str().unwrap().to_owned();
            let (size, sha) = if path == "weights.gguf" {
                (model.bytes, model.sha.to_owned())
            } else {
                (
                    files[&path].len() as u64,
                    hex::encode(Sha256::digest(&files[&path])),
                )
            };
            file["size"] = serde_json::json!(size);
            file["sha256"] = serde_json::json!(sha);
            let size_text = size.to_string();
            for part in [
                path.as_bytes(),
                b"\0",
                sha.as_bytes(),
                b"\0",
                size_text.as_bytes(),
                b"\0",
            ] {
                digest.update(part);
            }
        }
        object["content_digest"] = serde_json::json!(format!("sha256:{:x}", digest.finalize()));
        files.insert(
            "_elastos_object.json".into(),
            serde_json::to_vec(object).unwrap(),
        );
        assert!(files.values().map(Vec::len).sum::<usize>() <= 65536);
        (payload, files)
    }

    async fn install_engine(&self, data: &Path, root: &Path, deadline: Instant) -> EngineCleanup {
        let mut bytes = Vec::new();
        proof_file(&self.engine_data.join("components.json"), 4 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .unwrap();
        let manifest: crate::setup::ComponentsManifest = serde_json::from_slice(&bytes).unwrap();
        let identity =
            crate::setup::verified_local_model_engine(&self.engine_data, &manifest).unwrap();
        assert_eq!(
            identity.sha256.trim_start_matches("sha256:"),
            pinned_engine_sha(&crate::setup::detect_platform())
        );
        let source = identity.path.parent().unwrap();
        let receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(source.join(".elastos-engine.json")).unwrap())
                .unwrap();
        assert_eq!(receipt["version"], "b10516");
        // This pinned bundle has a root executable. Bound copies from its
        // verified receipt, including directories and relative library links.
        // The Linux bundle carries one CPU backend library per
        // microarchitecture, so its bound is wider than the Metal bundle.
        let entries = receipt["entries"].as_array().unwrap();
        assert!(entries.len() <= 128);
        let total = entries
            .iter()
            .filter(|e| e["type"] == "file")
            .map(|e| {
                fs::metadata(source.join(e["path"].as_str().unwrap()))
                    .unwrap()
                    .len()
            })
            .sum::<u64>();
        assert!(total + 256 * 1024 <= 96 * 1024 * 1024);
        let bundle = data.join("proof-engine");
        let mut copy = Command::new("/bin/cp");
        copy.args(["-pR"])
            .arg(source)
            .arg(&bundle)
            .stdin(Stdio::null());
        let cleanup = EngineCleanup(bundle.clone());
        run(copy, root, deadline).await;
        change_config(data, |config| {
            let mut component = serde_json::to_value(&manifest.external["llama-server"]).unwrap();
            component["platforms"][crate::setup::detect_platform()]["install_path"] =
                serde_json::json!("proof-engine");
            config["external"]["llama-server"] = component;
        });
        let current: crate::setup::ComponentsManifest =
            serde_json::from_slice(&fs::read(data.join("components.json")).unwrap()).unwrap();
        assert_eq!(
            crate::setup::verified_local_model_engine(data, &current)
                .unwrap()
                .sha256,
            identity.sha256
        );
        cleanup
    }
}

struct EngineCleanup(PathBuf);
impl Drop for EngineCleanup {
    fn drop(&mut self) {
        fn writable_dirs(path: &Path) {
            if fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
                for entry in fs::read_dir(path).unwrap() {
                    writable_dirs(&entry.unwrap().path());
                }
            }
        }
        // Only the exact copied fixture tree; TempDir removes it after children.
        writable_dirs(&self.0);
    }
}

fn proof_rss_kib() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    assert_eq!(
        unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
        0
    );
    let maxrss = unsafe { usage.assume_init() }.ru_maxrss as u64;
    // macOS reports bytes; Linux already reports KiB.
    if cfg!(target_os = "macos") {
        maxrss / 1024
    } else {
        maxrss
    }
}

#[tokio::test]
#[ignore = "requires exact read-only Qwen weights, verified engine and native provider"]
async fn model_native_qwen_exact_profile_lifecycle() {
    native_exact_profile_lifecycle(&QWEN).await;
}

#[tokio::test]
#[ignore = "requires exact read-only SmolLM2 weights, verified engine and native provider"]
async fn model_native_smollm2_exact_profile_lifecycle() {
    native_exact_profile_lifecycle(&SMOLLM2).await;
}

async fn native_exact_profile_lifecycle(model: &'static PinnedModel) {
    use elastos_runtime::provider::{bridge::ProviderConfig, ProviderBridge};
    let platform = crate::setup::detect_platform();
    let weights = model.env_path("PATH").canonicalize().unwrap();
    assert_eq!(
        proof_file(&weights, model.bytes).metadata().unwrap().len(),
        model.bytes
    );
    let engine_data = model.env_path("ENGINE_DATA");
    let mut manifest_bytes = Vec::new();
    proof_file(&engine_data.join("components.json"), 4 * 1024 * 1024)
        .read_to_end(&mut manifest_bytes)
        .unwrap();
    let manifest: crate::setup::ComponentsManifest =
        serde_json::from_slice(&manifest_bytes).unwrap();
    let engine = crate::setup::verified_local_model_engine(&engine_data, &manifest).unwrap();
    assert_eq!(
        engine.sha256.trim_start_matches("sha256:"),
        pinned_engine_sha(&platform)
    );
    let engine_path = engine.path.canonicalize().unwrap();
    let base = weights
        .ancestors()
        .find(|base| engine_path.starts_with(base))
        .unwrap();
    assert!(
        base.parent().is_some(),
        "proof must not admit filesystem root"
    );
    // This is native startup proof, not Content delivery/admission. Reuse the
    // exact Runtime policy while granting read-only access to just two files.
    let name = model.capsule_name;
    let mut offer = bound_model_offer(
        "native-only-fixture",
        "weights.gguf",
        model.sha,
        &engine.receipt_sha256,
        &engine.sha256,
        &format!("{name} native"),
    )
    .unwrap();
    offer.as_object_mut().unwrap().remove("stream_output");
    offer["policy"].as_object_mut().unwrap().remove("schema");
    offer["enabled"] = serde_json::json!(true);
    offer["adapter"] = serde_json::json!({
        "kind":"local_llama_cpp_text",
        "engine":{"path":engine_path,"sha256":engine.sha256},
        "model":{"path":weights,"sha256":format!("sha256:{}", model.sha)},
        "settings":local_model_startup_profile(&platform).unwrap()
    });
    assert_eq!(offer["policy"]["runtime_ms_limit"], 120000);
    let id = offer["id"].as_str().unwrap().to_owned();
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let cleanup_path = root.path().to_owned();
    eprintln!("{name} native proof journal: {}", cleanup_path.display());
    let config = ProviderConfig {
        base_path: base.to_string_lossy().into_owned(),
        allowed_paths: [&weights, &engine_path]
            .into_iter()
            .map(|p| p.strip_prefix(base).unwrap().to_string_lossy().into_owned())
            .collect(),
        read_only: true,
        extra: serde_json::json!({"provider_id":"model-provider","journal_dir":root.path(),
            "offers":[offer],"runtime_admitted_offers":[{"offer_id":id}]}),
        ..Default::default()
    };
    let started = Instant::now();
    let binary = PathBuf::from(
        std::env::var_os("ELASTOS_TEST_MODEL_PROVIDER_PATH")
            .expect("explicit ELASTOS_TEST_MODEL_PROVIDER_PATH proof prerequisite"),
    );
    let bridge = Arc::new(
        ProviderBridge::spawn(&binary, config.clone())
            .await
            .unwrap(),
    );
    eprintln!(
        "{name} native Init elapsed_ms={}",
        started.elapsed().as_millis()
    );
    let children = Arc::new(Mutex::new(vec![bridge.clone()]));
    let task_children = children.clone();
    // Two independently bounded 120 s runs, plus startup/replay/cleanup margin.
    let mut task = tokio::spawn(async move {
        let proof = pinned_runs_and_active_cancel(
            model,
            bridge.clone(),
            id,
            Instant::now() + Duration::from_secs(300),
        )
        .await;
        bridge
            .shutdown()
            .await
            .expect("native model provider reaped");
        let restarted = Arc::new(ProviderBridge::spawn(&binary, config).await.unwrap());
        task_children.lock().unwrap().push(restarted.clone());
        assert_pinned_replay(&restarted, &proof).await;
        proof.active_delta_bytes
    });
    let result = tokio::time::timeout(Duration::from_secs(300), &mut task).await;
    if result.is_err() {
        task.abort();
        let _ = task.await;
    }
    eprintln!(
        "{name} native terminal elapsed_ms={}",
        started.elapsed().as_millis()
    );
    let mut shutdowns = Vec::new();
    let owned = std::mem::take(&mut *children.lock().unwrap());
    for child in owned {
        shutdowns.push(child.shutdown().await);
    }
    let cleanup = root.close();
    for shutdown in shutdowns {
        shutdown.expect("native proof provider shutdown/reap");
    }
    cleanup.expect("native proof journal cleanup");
    assert!(!cleanup_path.exists());
    eprintln!("{name} native proof provider reaped and journal removed");
    let delta_bytes = result
        .expect("native proof waiter deadline")
        .expect("native proof task");
    assert!(delta_bytes > 0);
    eprintln!("{name} native active_delta_bytes={delta_bytes}; exact restart replay passed");
}

async fn pinned_first_reply(
    bridge: Arc<elastos_runtime::provider::ProviderBridge>,
    offer: String,
    deadline: Instant,
) -> (serde_json::Value, serde_json::Value) {
    use elastos_model_contract::{
        model_input_hash, RuntimeAccessBinding, RuntimeCreateBinding,
        RUNTIME_ACCESS_BINDING_SCHEMA, RUNTIME_CREATE_BINDING_SCHEMA,
    };
    let ctx = context();
    let input = serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"Reply with exactly one word: ready."});
    let binding = RuntimeCreateBinding {
        schema: RUNTIME_CREATE_BINDING_SCHEMA.into(),
        principal_id: ctx.principal_id.clone(),
        session_id: ctx.session_id.clone(),
        capsule_id: "assistant".into(),
        grant_id: ctx.grant_id.clone(),
        request_id: "pinned-proof-reply".into(),
        offer_id: offer.clone(),
        operation: "text.generate".into(),
        input_hash: model_input_hash(&input).unwrap(),
    };
    let request = serde_json::json!({"op":"runs_create","offer_id":offer,"operation":"text.generate","input":input,"runtime_binding":binding});
    let created = bridge.send_raw(&request).await.unwrap();
    assert_eq!(created["status"], "ok");
    let id = created["data"]["run_id"].as_str().unwrap();
    let access = RuntimeAccessBinding {
        schema: RUNTIME_ACCESS_BINDING_SCHEMA.into(),
        principal_id: binding.principal_id,
        session_id: binding.session_id,
        capsule_id: binding.capsule_id,
        grant_id: binding.grant_id,
        request_id: binding.request_id,
        run_id: id.into(),
    };
    let get = serde_json::json!({"op":"runs_get","run_id":id,"runtime_binding":access});
    let terminal = loop {
        assert!(Instant::now() < deadline);
        let result = bridge.send_raw(&get).await.unwrap();
        assert_eq!(result["status"], "ok");
        if !matches!(
            result["data"]["status"].as_str(),
            Some("prepared" | "running" | "reconciling")
        ) {
            break result;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    report_terminal(&terminal);
    (get, terminal)
}

fn report_terminal(terminal: &serde_json::Value) {
    // Preserve bounded public failure facts before cleanup; never dump paths,
    // prompts, provider config or the complete private response.
    let bounded = |value: &serde_json::Value| {
        value
            .as_str()
            .unwrap_or("")
            .chars()
            .take(256)
            .collect::<String>()
    };
    eprintln!(
        "pinned model terminal {}",
        serde_json::json!({
            "status": bounded(&terminal["data"]["status"]),
            "error_class": bounded(&terminal["data"]["terminal"]["error"]["class"]),
            "error_code": bounded(&terminal["data"]["terminal"]["error"]["code"]),
            "error_message": bounded(&terminal["data"]["terminal"]["error"]["message"]),
        })
    );
}

fn assert_pinned_reply(model: &PinnedModel, terminal: &serde_json::Value) {
    assert_eq!(terminal["data"]["status"], "completed");
    let text = terminal["data"]["terminal"]["output"]["text"]
        .as_str()
        .unwrap()
        .trim();
    // The reply is the only model output this proof records; keep it short.
    eprintln!(
        "{} completed reply chars={} head={:?}",
        model.capsule_name,
        text.chars().count(),
        text.chars().take(64).collect::<String>()
    );
    assert!((model.reply_accepts)(text), "reply rejected by pinned model check");
}

struct PinnedRunProof {
    replay: [(serde_json::Value, serde_json::Value); 4],
    active_delta_bytes: usize,
}

async fn pinned_runs_and_active_cancel(
    model: &PinnedModel,
    bridge: Arc<elastos_runtime::provider::ProviderBridge>,
    offer: String,
    deadline: Instant,
) -> PinnedRunProof {
    use elastos_model_contract::{
        model_input_hash, RuntimeAccessBinding, RuntimeCreateBinding,
        RUNTIME_ACCESS_BINDING_SCHEMA, RUNTIME_CREATE_BINDING_SCHEMA,
    };
    let (get, terminal) = pinned_first_reply(bridge.clone(), offer.clone(), deadline).await;
    assert_pinned_reply(model, &terminal);
    let ctx = context();
    let input = serde_json::json!({"schema":"elastos.model.input.text/v1",
        "prompt":model.long_prompt});
    let binding = RuntimeCreateBinding {
        schema: RUNTIME_CREATE_BINDING_SCHEMA.into(),
        principal_id: ctx.principal_id.clone(),
        session_id: ctx.session_id.clone(),
        capsule_id: "assistant".into(),
        grant_id: ctx.grant_id.clone(),
        request_id: "pinned-proof-active-cancel".into(),
        offer_id: offer.clone(),
        operation: "text.generate".into(),
        input_hash: model_input_hash(&input).unwrap(),
    };
    let created = bridge
        .send_raw(&serde_json::json!({"op":"runs_create",
        "offer_id":offer,"operation":"text.generate","input":input,"runtime_binding":binding}))
        .await
        .unwrap();
    assert_eq!(created["status"], "ok");
    let id = created["data"]["run_id"].as_str().unwrap();
    assert_ne!(id, get["run_id"].as_str().unwrap());
    let access = RuntimeAccessBinding {
        schema: RUNTIME_ACCESS_BINDING_SCHEMA.into(),
        principal_id: binding.principal_id,
        session_id: binding.session_id,
        capsule_id: binding.capsule_id,
        grant_id: binding.grant_id,
        request_id: binding.request_id,
        run_id: id.into(),
    };
    let cancelled_get = serde_json::json!({"op":"runs_get","run_id":id,"runtime_binding":access});
    let mut cursor = 0u64;
    let active_delta_bytes = loop {
        assert!(
            Instant::now() < deadline,
            "active pinned model delta observation deadline"
        );
        let page = bridge
            .send_raw(&serde_json::json!({"op":"runs_events",
            "run_id":id,"after_sequence":cursor,"runtime_binding":access}))
            .await
            .unwrap();
        assert_eq!(page["status"], "ok");
        assert_eq!(page["data"]["schema"], "elastos.model.run-events/v1");
        assert_eq!(page["data"]["run_id"], id);
        let next = page["data"]["next_cursor"].as_u64().unwrap();
        assert!(next >= cursor);
        cursor = next;
        let delta_bytes: usize = page["data"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"] == "text_delta")
            .map(|event| event["data"]["text"].as_str().unwrap().len())
            .sum();
        let current = bridge.send_raw(&cancelled_get).await.unwrap();
        assert_eq!(current["status"], "ok");
        if !matches!(
            current["data"]["status"].as_str(),
            Some("prepared" | "running")
        ) {
            report_terminal(&current);
        }
        assert!(
            matches!(
                current["data"]["status"].as_str(),
                Some("prepared" | "running")
            ),
            "active cancellation not observed before terminal status: {}",
            current["data"]["status"]
        );
        if delta_bytes > 0 && current["data"]["status"] == "running" {
            break delta_bytes;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let cancel = serde_json::json!({"op":"runs_cancel","run_id":id,"runtime_binding":access});
    let cancelled = bridge.send_raw(&cancel).await.unwrap();
    report_terminal(&cancelled);
    assert_eq!(cancelled["status"], "ok");
    assert!(
        matches!(
            cancelled["data"]["status"].as_str(),
            Some("reconciling" | "settlement_unknown")
        ),
        "active cancellation raced terminal completion: {}",
        cancelled["data"]["status"]
    );
    let cancelled_terminal = loop {
        assert!(
            Instant::now() < deadline,
            "cancellation settlement deadline"
        );
        let result = bridge.send_raw(&cancelled_get).await.unwrap();
        assert_eq!(result["status"], "ok");
        if !matches!(
            result["data"]["status"].as_str(),
            Some("prepared" | "running" | "reconciling")
        ) {
            break result;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    report_terminal(&cancelled_terminal);
    // Preserve the unresolved run outcome. HTTP disconnect does not prove that
    // backend execution stopped; later artifact removal needs physical close proof.
    assert_eq!(cancelled_terminal["data"]["status"], "settlement_unknown");
    assert_eq!(
        cancelled_terminal["data"]["terminal"]["error"]["class"],
        "settlement_unknown"
    );
    let events = serde_json::json!({"op":"runs_events","run_id":id,
        "after_sequence":0,"runtime_binding":access});
    let settled_events = bridge.send_raw(&events).await.unwrap();
    assert_eq!(settled_events["status"], "ok");
    assert_eq!(settled_events["data"]["has_more"], false);
    assert_eq!(
        settled_events["data"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"] == "dispatched")
            .count(),
        1
    );
    for _ in 0..2 {
        assert_eq!(bridge.send_raw(&cancel).await.unwrap(), cancelled_terminal);
        assert_eq!(
            bridge.send_raw(&cancelled_get).await.unwrap(),
            cancelled_terminal
        );
        assert_eq!(bridge.send_raw(&events).await.unwrap(), settled_events);
    }
    PinnedRunProof {
        replay: [
            (get, terminal),
            (cancelled_get, cancelled_terminal.clone()),
            (cancel, cancelled_terminal),
            (events, settled_events),
        ],
        active_delta_bytes,
    }
}

async fn assert_pinned_replay(
    bridge: &elastos_runtime::provider::ProviderBridge,
    proof: &PinnedRunProof,
) {
    let mut replies = Vec::new();
    for (query, _) in &proof.replay {
        replies.push(bridge.send_raw(query).await);
    }
    bridge
        .shutdown()
        .await
        .expect("restarted model provider reaped");
    for (reply, (_, expected)) in replies.into_iter().zip(&proof.replay) {
        assert_eq!(reply.unwrap(), *expected);
    }
}

async fn pinned_reply_and_restart(
    model: &PinnedModel,
    data: &Path,
    registry: &elastos_runtime::provider::ProviderRegistry,
    binary: &Path,
    cid: &str,
    deadline: Instant,
    children: &Arc<Mutex<Vec<Arc<elastos_runtime::provider::ProviderBridge>>>>,
) -> serde_json::Value {
    use elastos_runtime::provider::ProviderBridge;
    let (config, worker) = crate::api::model_provider_config(data, registry)
        .await
        .unwrap();
    let offer = config.extra["offers"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        config.extra["runtime_admitted_offers"],
        serde_json::json!([{"offer_id":offer}])
    );
    assert_eq!(
        config.extra["offers"][0]["adapter"]["settings"],
        local_model_startup_profile(&crate::setup::detect_platform()).unwrap(),
        "startup offer carries this host's Runtime-owned profile"
    );
    let bridge = Arc::new(ProviderBridge::spawn(binary, config.clone()).await.unwrap());
    children.lock().unwrap().push(bridge.clone());
    drop(worker);
    let proof = pinned_runs_and_active_cancel(model, bridge.clone(), offer.clone(), deadline).await;
    bridge
        .shutdown()
        .await
        .expect("pinned model provider shutdown/reap");
    children.lock().unwrap().clear();
    let (restarted_config, worker) = crate::api::model_provider_config(data, registry)
        .await
        .unwrap();
    assert_eq!(restarted_config.extra, config.extra);
    let restarted = Arc::new(
        ProviderBridge::spawn(binary, restarted_config)
            .await
            .unwrap(),
    );
    children.lock().unwrap().push(restarted.clone());
    drop(worker);
    assert_pinned_replay(&restarted, &proof).await;
    children.lock().unwrap().clear();
    assert!(Inventory::open(data, false)
        .unwrap()
        .load()
        .unwrap()
        .kept(&context().principal_id, cid));
    serde_json::json!({"offer_id":offer,"cid":cid,"reply_completed":true,"restart_replay_exact":true,
        "keep_persisted":true,"provider_shutdown_reap":true,"engine_process_group_absence":null,
        "active_text_delta_bytes":proof.active_delta_bytes,"active_cancel_unknown_replay":true,
        "per_run_backend_stop_confirmed":false})
}

#[tokio::test]
#[ignore = "requires explicit read-only Qwen/license/verified engine roots and native binaries; review footprint before running"]
async fn model_preparation_real_qwen_cold_reply_restart() {
    assert_eq!(crate::setup::detect_platform(), "darwin-arm64");
    preparation_process_with_model(true, Some(PinnedModelProof::from_env(&QWEN))).await;
}

#[tokio::test]
#[ignore = "requires explicit read-only SmolLM2/license/verified engine roots and native binaries; isolated small-model proof"]
async fn model_preparation_real_smollm2_cold_reply_restart() {
    // Any host with a Runtime-owned startup profile runs this small model.
    local_model_startup_profile(&crate::setup::detect_platform()).unwrap();
    preparation_process_with_model(true, Some(PinnedModelProof::from_env(&SMOLLM2))).await;
}
