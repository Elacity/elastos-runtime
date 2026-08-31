use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, Write};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const PROVIDER_ID: &str = "media-provider";
const PROTOCOL_VERSION: &str = "elastos.media-provider/v1";
const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const INIT_ERROR_CODE: &str = "invalid_config";
const REQUEST_ERROR_CODE: &str = "invalid_request";
const INTERNAL_ERROR_CODE: &str = "internal_error";
const OPERATION_ID_HEX_LEN: usize = 64;
const MAX_PROVIDER_FRAME_BYTES: usize = 16 * 1024;
const MAX_TIMEOUT_MS: u64 = 3_600_000;
const MAX_STDIO_BYTES: usize = 1024 * 1024;
const MAX_INPUT_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_OUTPUT_PART_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DURATION_SECS: u64 = 1_800;
const MAX_SOURCE_WIDTH: u32 = 3_840;
const MAX_SOURCE_HEIGHT: u32 = 2_160;
const MAX_SOURCE_FPS: u32 = 60;
const MAX_SEGMENT_COUNT: usize = 512;
const MAX_TOTAL_OUTPUT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const OUTPUT_SCHEMA_V1: &str = "elastos.media-provider.prepared-media/v1";
const OUTPUT_PROFILE_BROWSER_FMP4_H264_V1: &str = "browser_fmp4_h264_v1";
const OUTPUT_MIME_TYPE_V1: &str = "video/mp4";
const OUTPUT_CODECS_V1: &str = "avc1.640028";
const OUTPUT_SEGMENT_DURATION_SECS_V1: &str = "4";

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum ControlRequest {
    Init { config: Value },
    Status,
    Shutdown,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum MediaProviderRequest {
    Prepare { operation_id: String },
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum ProviderResponse {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Error {
        code: &'static str,
        message: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
}

impl ProviderResponse {
    fn ok(data: Value) -> Self {
        Self::Ok { data: Some(data) }
    }

    fn empty_ok() -> Self {
        Self::Ok { data: None }
    }

    fn error(code: &'static str, message: &'static str) -> Self {
        Self::Error {
            code,
            message,
            data: None,
        }
    }

    fn settled_error(code: &'static str, message: &'static str, settled: bool) -> Self {
        Self::Error {
            code,
            message,
            data: Some(json!({"operation_settled": settled})),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InitConfig {
    #[serde(default)]
    base_path: String,
    #[serde(default)]
    allowed_paths: Vec<String>,
    #[serde(default)]
    read_only: bool,
    #[serde(default)]
    encryption_key: String,
    extra: MediaInitExtraConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MediaInitExtraConfig {
    provider_id: String,
    staging_root: String,
    ffmpeg_path: String,
    ffprobe_path: String,
    output_profile: String,
    timeout_ms: u64,
    max_stdio_bytes: usize,
    max_input_bytes: u64,
    max_output_part_bytes: u64,
    max_duration_secs: u64,
    max_source_width: u32,
    max_source_height: u32,
    max_source_fps: u32,
    max_segment_count: usize,
    max_total_output_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct PreparedMediaOutput {
    schema: &'static str,
    mime_type: String,
    codecs: String,
}

#[derive(Debug, Clone)]
struct ConfiguredMediaProvider {
    staging_root: PathBuf,
    ffmpeg_path: PathBuf,
    ffprobe_path: PathBuf,
    timeout: Duration,
    max_stdio_bytes: usize,
    max_input_bytes: u64,
    max_output_part_bytes: u64,
    max_duration_secs: u64,
    max_source_width: u32,
    max_source_height: u32,
    max_source_fps: u32,
    max_segment_count: usize,
    max_total_output_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct ProbeOutput {
    streams: Vec<ProbeStream>,
    format: ProbeFormat,
    #[serde(flatten)]
    _extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    #[serde(flatten)]
    _extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
    #[serde(flatten)]
    _extra: BTreeMap<String, Value>,
}

#[derive(Debug)]
struct BoundedCommandOutput {
    stdout: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
enum CommandLifecycleEvent {
    Spawned {
        child_id: u32,
    },
    Reaped {
        child_id: u32,
        code: Option<i32>,
        signal: Option<i32>,
    },
}

#[derive(Debug)]
struct PreparedOutputMonitor<'a> {
    root: &'a Path,
    max_total_output_bytes: u64,
    max_entry_count: usize,
}

#[derive(Debug)]
struct ValidatedProbeOutput {
    duration_secs: f64,
}

#[derive(Debug)]
struct MonitoredOutputUsage {
    total_bytes: u64,
    entry_count: usize,
}

pub struct MediaProvider {
    state: Option<ConfiguredMediaProvider>,
}

impl Default for MediaProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaProvider {
    pub fn new() -> Self {
        Self { state: None }
    }

    fn handle_frame(&mut self, frame: &[u8]) -> (ProviderResponse, bool) {
        let mut value = match serde_json::from_slice::<Value>(frame) {
            Ok(value) => value,
            Err(_) => return (invalid_request(), false),
        };
        let op = value.get("op").and_then(Value::as_str).map(str::to_owned);
        let envelope_present = strip_runtime_invocation_envelope(
            &mut value,
            "media",
            op.as_deref().unwrap_or_default(),
        )
        .is_ok();
        match op.as_deref() {
            Some("init" | "status" | "shutdown") => {
                if !control_request_has_exact_fields(&value, op.as_deref().unwrap_or_default()) {
                    return (invalid_request(), false);
                }
                match serde_json::from_value::<ControlRequest>(value) {
                    Ok(ControlRequest::Init { config }) => (self.init(config), false),
                    Ok(ControlRequest::Status) => (self.status(), false),
                    Ok(ControlRequest::Shutdown) => (ProviderResponse::empty_ok(), true),
                    Err(_) => (invalid_request(), false),
                }
            }
            Some("prepare") => {
                if !envelope_present {
                    return (invalid_request(), false);
                }
                match serde_json::from_value::<MediaProviderRequest>(value) {
                    Ok(MediaProviderRequest::Prepare { operation_id }) => {
                        (self.prepare(&operation_id), false)
                    }
                    Err(_) => (invalid_request(), false),
                }
            }
            _ => (invalid_request(), false),
        }
    }

    fn init(&mut self, config: Value) -> ProviderResponse {
        self.state = None;
        match load_provider_state(config) {
            Ok(state) => {
                self.state = Some(state);
                self.status()
            }
            Err(()) => {
                ProviderResponse::error(INIT_ERROR_CODE, "media provider configuration is invalid")
            }
        }
    }

    fn status(&self) -> ProviderResponse {
        ProviderResponse::ok(json!({
            "provider": PROVIDER_ID,
            "protocol_version": PROTOCOL_VERSION,
            "version": PROVIDER_VERSION,
            "configured": self.state.is_some(),
            "supported_operations": ["status", "prepare"],
        }))
    }

    fn prepare(&self, operation_id: &str) -> ProviderResponse {
        let Some(state) = &self.state else {
            return ProviderResponse::error(
                INTERNAL_ERROR_CODE,
                "media provider is not configured",
            );
        };
        let mut lifecycle = Vec::new();
        let result = validate_operation_id(operation_id).and_then(|()| {
            run_prepare_observed(state, operation_id, &mut |event| lifecycle.push(event))
                .map_err(|_| ())
        });
        let operation_settled = command_lifecycle_is_settled(&lifecycle);
        match result {
            Ok(output) if operation_settled => {
                ProviderResponse::ok(serde_json::to_value(output).unwrap_or(Value::Null))
            }
            Ok(_) | Err(()) => ProviderResponse::settled_error(
                INTERNAL_ERROR_CODE,
                if operation_settled {
                    "media preparation failed"
                } else {
                    "media preparation settlement is unknown"
                },
                operation_settled,
            ),
        }
    }
}

fn load_provider_state(config: Value) -> Result<ConfiguredMediaProvider, ()> {
    let config = serde_json::from_value::<InitConfig>(config).map_err(|_| ())?;
    if !config.base_path.is_empty()
        || !config.allowed_paths.is_empty()
        || config.read_only
        || !config.encryption_key.is_empty()
    {
        return Err(());
    }
    let extra = config.extra;
    if extra.provider_id != PROVIDER_ID
        || extra.timeout_ms == 0
        || extra.max_stdio_bytes == 0
        || extra.max_input_bytes == 0
        || extra.max_output_part_bytes == 0
        || extra.max_duration_secs == 0
        || extra.max_source_width == 0
        || extra.max_source_height == 0
        || extra.max_source_fps == 0
        || extra.max_segment_count == 0
        || extra.max_total_output_bytes == 0
        || extra.timeout_ms > MAX_TIMEOUT_MS
        || extra.max_stdio_bytes > MAX_STDIO_BYTES
        || extra.max_input_bytes > MAX_INPUT_BYTES
        || extra.max_output_part_bytes > MAX_OUTPUT_PART_BYTES
        || extra.max_duration_secs > MAX_DURATION_SECS
        || extra.max_source_width > MAX_SOURCE_WIDTH
        || extra.max_source_height > MAX_SOURCE_HEIGHT
        || extra.max_source_fps > MAX_SOURCE_FPS
        || extra.max_segment_count > MAX_SEGMENT_COUNT
        || extra.max_total_output_bytes > MAX_TOTAL_OUTPUT_BYTES
        || extra.max_total_output_bytes < extra.max_output_part_bytes
        || extra.output_profile != OUTPUT_PROFILE_BROWSER_FMP4_H264_V1
    {
        return Err(());
    }
    let staging_root = PathBuf::from(extra.staging_root);
    let ffmpeg_path = PathBuf::from(extra.ffmpeg_path);
    let ffprobe_path = PathBuf::from(extra.ffprobe_path);
    if !staging_root.is_absolute() || !ffmpeg_path.is_absolute() || !ffprobe_path.is_absolute() {
        return Err(());
    }
    validate_owner_only_dir(&staging_root).map_err(|_| ())?;
    validate_regular_executable_path(&ffmpeg_path).map_err(|_| ())?;
    validate_regular_executable_path(&ffprobe_path).map_err(|_| ())?;
    Ok(ConfiguredMediaProvider {
        staging_root,
        ffmpeg_path,
        ffprobe_path,
        timeout: Duration::from_millis(extra.timeout_ms),
        max_stdio_bytes: extra.max_stdio_bytes,
        max_input_bytes: extra.max_input_bytes,
        max_output_part_bytes: extra.max_output_part_bytes,
        max_duration_secs: extra.max_duration_secs,
        max_source_width: extra.max_source_width,
        max_source_height: extra.max_source_height,
        max_source_fps: extra.max_source_fps,
        max_segment_count: extra.max_segment_count,
        max_total_output_bytes: extra.max_total_output_bytes,
    })
}

fn run_prepare_observed(
    state: &ConfiguredMediaProvider,
    operation_id: &str,
    observe_lifecycle: &mut dyn FnMut(CommandLifecycleEvent),
) -> Result<PreparedMediaOutput, ()> {
    let operation_root = state.staging_root.join(operation_id);
    validate_operation_root(&state.staging_root, &operation_root)?;
    let input_path = operation_root.join("input.bin");
    validate_regular_file(&input_path, true, state.max_input_bytes)?;
    let prepared_dir = operation_root.join("prepared");
    if prepared_dir.exists() {
        fs::remove_dir_all(&prepared_dir).map_err(|_| ())?;
    }
    fs::create_dir(&prepared_dir).map_err(|_| ())?;
    set_owner_only_dir_permissions(&prepared_dir).map_err(|_| ())?;
    let segments_dir = prepared_dir.join("segments");
    fs::create_dir(&segments_dir).map_err(|_| ())?;
    set_owner_only_dir_permissions(&segments_dir).map_err(|_| ())?;
    let probe = run_ffprobe_observed(state, &input_path, observe_lifecycle)?;
    let expected_segments = segment_count_for_duration(probe.duration_secs)?;
    if expected_segments > state.max_segment_count {
        return Err(());
    }
    run_ffmpeg_observed(state, &input_path, &prepared_dir, observe_lifecycle)?;
    let init_len = validate_regular_file_len(
        &prepared_dir.join("init.mp4"),
        false,
        state.max_output_part_bytes,
    )?;
    let segments_len = validate_segments_output(
        &segments_dir,
        state.max_output_part_bytes,
        state.max_segment_count,
    )?;
    if init_len.checked_add(segments_len).ok_or(())? > state.max_total_output_bytes {
        return Err(());
    }
    Ok(PreparedMediaOutput {
        schema: OUTPUT_SCHEMA_V1,
        mime_type: OUTPUT_MIME_TYPE_V1.to_string(),
        codecs: OUTPUT_CODECS_V1.to_string(),
    })
}

fn run_ffprobe_observed(
    state: &ConfiguredMediaProvider,
    input_path: &Path,
    observe_lifecycle: &mut dyn FnMut(CommandLifecycleEvent),
) -> Result<ValidatedProbeOutput, ()> {
    let mut cmd = Command::new(&state.ffprobe_path);
    cmd.arg("-v")
        .arg("error")
        .arg("-print_format")
        .arg("json")
        .arg("-show_entries")
        .arg("format=duration:stream=codec_type,width,height,avg_frame_rate,r_frame_rate")
        .arg(input_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = run_bounded_command_observed(
        cmd,
        state.timeout,
        state.max_stdio_bytes,
        None,
        observe_lifecycle,
    )?;
    let probe = serde_json::from_slice::<ProbeOutput>(&output.stdout).map_err(|_| ())?;
    validate_probe_output(&probe, state)
}

fn run_ffmpeg_observed(
    state: &ConfiguredMediaProvider,
    input_path: &Path,
    prepared_dir: &Path,
    observe_lifecycle: &mut dyn FnMut(CommandLifecycleEvent),
) -> Result<(), ()> {
    let manifest_path = prepared_dir.join("manifest.mpd");
    let media_seg_name = {
        let mut name = OsString::from("segments/");
        name.push("$Number%08d$.m4s");
        name
    };
    let mut cmd = Command::new(&state.ffmpeg_path);
    cmd.arg("-nostdin")
        .arg("-v")
        .arg("error")
        .arg("-y")
        .arg("-i")
        .arg(input_path)
        .arg("-map")
        .arg("0:v:0")
        .arg("-an")
        .arg("-c:v")
        .arg("libx264")
        .arg("-profile:v")
        .arg("high")
        .arg("-level:v")
        .arg("4.0")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-r")
        .arg("30")
        .arg("-vf")
        .arg("scale=w=min(iw\\,1920):h=min(ih\\,1080):force_original_aspect_ratio=decrease:force_divisible_by=2,fps=30")
        .arg("-g")
        .arg("120")
        .arg("-keyint_min")
        .arg("120")
        .arg("-sc_threshold")
        .arg("0")
        .arg("-preset")
        .arg("veryfast")
        .arg("-crf")
        .arg("28")
        .arg("-movflags")
        .arg("+frag_keyframe+empty_moov+default_base_moof+separate_moof")
        .arg("-f")
        .arg("dash")
        .arg("-seg_duration")
        .arg(OUTPUT_SEGMENT_DURATION_SECS_V1)
        .arg("-streaming")
        .arg("1")
        .arg("-use_timeline")
        .arg("0")
        .arg("-use_template")
        .arg("1")
        .arg("-start_number")
        .arg("0")
        .arg("-init_seg_name")
        .arg("init.mp4")
        .arg("-media_seg_name")
        .arg(media_seg_name)
        .arg(&manifest_path)
        .current_dir(prepared_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    run_bounded_command_observed(
        cmd,
        state.timeout,
        state.max_stdio_bytes,
        Some(PreparedOutputMonitor {
            root: prepared_dir,
            max_total_output_bytes: state.max_total_output_bytes,
            max_entry_count: state.max_segment_count.checked_add(3).ok_or(())?,
        }),
        observe_lifecycle,
    )
    .map(|_| ())?;
    if manifest_path.exists() {
        fs::remove_file(&manifest_path).map_err(|_| ())?;
    }
    Ok(())
}

fn run_bounded_command_observed(
    mut cmd: Command,
    timeout: Duration,
    max_stdio_bytes: usize,
    monitor: Option<PreparedOutputMonitor<'_>>,
    observe_lifecycle: &mut dyn FnMut(CommandLifecycleEvent),
) -> Result<BoundedCommandOutput, ()> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|_| ())?;
    observe_lifecycle(CommandLifecycleEvent::Spawned {
        child_id: child.id(),
    });
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate_child(&mut child, observe_lifecycle)?;
            return Err(());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            terminate_child(&mut child, observe_lifecycle)?;
            return Err(());
        }
    };
    let stdout_thread = spawn_stdio_reader(stdout, max_stdio_bytes);
    let stderr_thread = spawn_stdio_reader(stderr, max_stdio_bytes);
    wait_for_child(
        &mut child,
        timeout,
        stdout_thread,
        stderr_thread,
        monitor,
        observe_lifecycle,
    )
}

fn wait_for_child(
    child: &mut Child,
    timeout: Duration,
    stdout_thread: thread::JoinHandle<Result<Vec<u8>, ()>>,
    stderr_thread: thread::JoinHandle<Result<Vec<u8>, ()>>,
    monitor: Option<PreparedOutputMonitor<'_>>,
    observe_lifecycle: &mut dyn FnMut(CommandLifecycleEvent),
) -> Result<BoundedCommandOutput, ()> {
    let deadline = match Instant::now().checked_add(timeout) {
        Some(deadline) => deadline,
        None => {
            terminate_and_join(
                child,
                Some(stdout_thread),
                Some(stderr_thread),
                observe_lifecycle,
            )?;
            return Err(());
        }
    };
    let mut stdout_thread = Some(stdout_thread);
    let mut stderr_thread = Some(stderr_thread);
    let mut stdout_output = None;
    let mut stderr_output = None;
    loop {
        if let Some(reader) = stdout_thread.as_ref() {
            if reader.is_finished() {
                match stdout_thread.take().unwrap().join().map_err(|_| ())? {
                    Ok(stdout) => stdout_output = Some(stdout),
                    Err(()) => {
                        terminate_and_join(child, None, stderr_thread.take(), observe_lifecycle)?;
                        return Err(());
                    }
                }
            }
        }
        if let Some(reader) = stderr_thread.as_ref() {
            if reader.is_finished() {
                match stderr_thread.take().unwrap().join().map_err(|_| ())? {
                    Ok(stderr) => stderr_output = Some(stderr),
                    Err(()) => {
                        terminate_and_join(child, stdout_thread.take(), None, observe_lifecycle)?;
                        return Err(());
                    }
                }
            }
        }
        if let Some(monitor) = &monitor {
            match monitored_output_usage(monitor.root) {
                Ok(usage)
                    if usage.total_bytes > monitor.max_total_output_bytes
                        || usage.entry_count > monitor.max_entry_count =>
                {
                    terminate_and_join(
                        child,
                        stdout_thread.take(),
                        stderr_thread.take(),
                        observe_lifecycle,
                    )?;
                    return Err(());
                }
                Ok(_) => {}
                Err(()) => {
                    terminate_and_join(
                        child,
                        stdout_thread.take(),
                        stderr_thread.take(),
                        observe_lifecycle,
                    )?;
                    return Err(());
                }
            }
        }
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(_) => {
                terminate_and_join(
                    child,
                    stdout_thread.take(),
                    stderr_thread.take(),
                    observe_lifecycle,
                )?;
                return Err(());
            }
        };
        match status {
            Some(status) => {
                observe_reaped_child(child.id(), &status, observe_lifecycle);
                let stdout = match stdout_output {
                    Some(stdout) => stdout,
                    None => stdout_thread.take().unwrap().join().map_err(|_| ())??,
                };
                let _stderr = match stderr_output {
                    Some(stderr) => stderr,
                    None => stderr_thread.take().unwrap().join().map_err(|_| ())??,
                };
                return if status.success() {
                    Ok(BoundedCommandOutput { stdout })
                } else {
                    Err(())
                };
            }
            None if Instant::now() >= deadline => {
                terminate_and_join(
                    child,
                    stdout_thread.take(),
                    stderr_thread.take(),
                    observe_lifecycle,
                )?;
                return Err(());
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn spawn_stdio_reader<T>(
    mut stream: T,
    max_stdio_bytes: usize,
) -> thread::JoinHandle<Result<Vec<u8>, ()>>
where
    T: io::Read + Send + 'static,
{
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let read = stream.read(&mut buffer).map_err(|_| ())?;
            if read == 0 {
                return Ok(output);
            }
            if output.len().saturating_add(read) > max_stdio_bytes {
                return Err(());
            }
            output.extend_from_slice(&buffer[..read]);
        }
    })
}

fn terminate_child(
    child: &mut Child,
    observe_lifecycle: &mut dyn FnMut(CommandLifecycleEvent),
) -> Result<(), ()> {
    let child_id = child.id();
    let _ = child.kill();
    let status = child.wait().map_err(|_| ())?;
    observe_reaped_child(child_id, &status, observe_lifecycle);
    Ok(())
}

fn observe_reaped_child(
    child_id: u32,
    status: &ExitStatus,
    observe_lifecycle: &mut dyn FnMut(CommandLifecycleEvent),
) {
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt as _;
        status.signal()
    };
    #[cfg(not(unix))]
    let signal = None;
    observe_lifecycle(CommandLifecycleEvent::Reaped {
        child_id,
        code: status.code(),
        signal,
    });
}

fn command_lifecycle_is_settled(events: &[CommandLifecycleEvent]) -> bool {
    let mut active = BTreeMap::new();
    for event in events {
        match event {
            CommandLifecycleEvent::Spawned { child_id } => {
                if active.insert(*child_id, ()).is_some() {
                    return false;
                }
            }
            CommandLifecycleEvent::Reaped { child_id, .. } => {
                if active.remove(child_id).is_none() {
                    return false;
                }
            }
        }
    }
    active.is_empty()
}

fn terminate_and_join(
    child: &mut Child,
    stdout_thread: Option<thread::JoinHandle<Result<Vec<u8>, ()>>>,
    stderr_thread: Option<thread::JoinHandle<Result<Vec<u8>, ()>>>,
    observe_lifecycle: &mut dyn FnMut(CommandLifecycleEvent),
) -> Result<(), ()> {
    terminate_child(child, observe_lifecycle)?;
    if let Some(stdout_thread) = stdout_thread {
        let _ = stdout_thread.join();
    }
    if let Some(stderr_thread) = stderr_thread {
        let _ = stderr_thread.join();
    }
    Ok(())
}

fn validate_segments_output(
    segments_dir: &Path,
    max_output_part_bytes: u64,
    max_segment_count: usize,
) -> Result<u64, ()> {
    let mut total_bytes = 0u64;
    let mut indexes = Vec::new();
    for entry in fs::read_dir(segments_dir).map_err(|_| ())? {
        let entry = entry.map_err(|_| ())?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        indexes.push(parse_segment_index(&name)?);
        total_bytes = total_bytes
            .checked_add(validate_regular_file_len(
                &path,
                true,
                max_output_part_bytes,
            )?)
            .ok_or(())?;
    }
    indexes.sort_unstable();
    if indexes.is_empty() || indexes.len() > max_segment_count {
        return Err(());
    }
    for (expected, actual) in indexes.into_iter().enumerate() {
        if actual != expected {
            return Err(());
        }
    }
    Ok(total_bytes)
}

fn validate_operation_root(staging_root: &Path, operation_root: &Path) -> Result<(), ()> {
    let canonical = staging_root.canonicalize().map_err(|_| ())?;
    let metadata = fs::symlink_metadata(operation_root).map_err(|_| ())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(());
    }
    #[cfg(unix)]
    validate_owner_only_dir(operation_root).map_err(|_| ())?;
    let canonical_operation_root = operation_root.canonicalize().map_err(|_| ())?;
    if !canonical_operation_root.starts_with(&canonical) {
        return Err(());
    }
    Ok(())
}

fn validate_regular_executable_path(path: &Path) -> Result<(), ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(());
    }
    #[cfg(unix)]
    {
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o022 != 0 || mode & 0o111 == 0 {
            return Err(());
        }
    }
    Ok(())
}

fn validate_regular_file_len(
    path: &Path,
    require_non_empty: bool,
    max_bytes: u64,
) -> Result<u64, ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(());
    }
    #[cfg(unix)]
    {
        if metadata.nlink() != 1 {
            return Err(());
        }
    }
    if metadata.len() > max_bytes || (require_non_empty && metadata.len() == 0) {
        return Err(());
    }
    Ok(metadata.len())
}

fn validate_regular_file(path: &Path, require_non_empty: bool, max_bytes: u64) -> Result<(), ()> {
    validate_regular_file_len(path, require_non_empty, max_bytes).map(|_| ())
}

fn validate_owner_only_dir(path: &Path) -> Result<(), ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(());
    }
    #[cfg(unix)]
    {
        let mode = metadata.permissions().mode() & 0o777;
        if metadata.uid() != unsafe { libc::geteuid() } || mode != 0o700 {
            return Err(());
        }
    }
    Ok(())
}

fn set_owner_only_dir_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn validate_probe_output(
    probe: &ProbeOutput,
    state: &ConfiguredMediaProvider,
) -> Result<ValidatedProbeOutput, ()> {
    let stream = probe
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("video"))
        .ok_or(())?;
    let width = stream.width.ok_or(())?;
    let height = stream.height.ok_or(())?;
    if width == 0
        || height == 0
        || width > state.max_source_width
        || height > state.max_source_height
    {
        return Err(());
    }
    let duration = parse_finite_decimal(probe.format.duration.as_deref().ok_or(())?)?;
    if duration <= 0.0 || duration > state.max_duration_secs as f64 {
        return Err(());
    }
    let fps = parse_frame_rate(
        stream.avg_frame_rate.as_deref(),
        stream.r_frame_rate.as_deref(),
    )?;
    if fps <= 0.0 || fps > state.max_source_fps as f64 {
        return Err(());
    }
    Ok(ValidatedProbeOutput {
        duration_secs: duration,
    })
}

fn parse_finite_decimal(value: &str) -> Result<f64, ()> {
    let parsed = value.parse::<f64>().map_err(|_| ())?;
    if parsed.is_finite() {
        Ok(parsed)
    } else {
        Err(())
    }
}

fn parse_frame_rate(avg_frame_rate: Option<&str>, r_frame_rate: Option<&str>) -> Result<f64, ()> {
    for candidate in [avg_frame_rate, r_frame_rate] {
        let Some(candidate) = candidate else {
            continue;
        };
        if let Ok(parsed) = parse_rate(candidate) {
            if parsed > 0.0 {
                return Ok(parsed);
            }
        }
    }
    Err(())
}

fn parse_rate(value: &str) -> Result<f64, ()> {
    let (numerator, denominator) = value.split_once('/').ok_or(())?;
    let numerator = numerator.parse::<f64>().map_err(|_| ())?;
    let denominator = denominator.parse::<f64>().map_err(|_| ())?;
    if !numerator.is_finite() || !denominator.is_finite() || denominator <= 0.0 {
        return Err(());
    }
    Ok(numerator / denominator)
}

fn parse_segment_index(name: &str) -> Result<usize, ()> {
    let stem = name.strip_suffix(".m4s").ok_or(())?;
    if stem.len() != 8 || !stem.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(());
    }
    stem.parse::<usize>().map_err(|_| ())
}

fn segment_count_for_duration(duration_secs: f64) -> Result<usize, ()> {
    if !duration_secs.is_finite() || duration_secs <= 0.0 {
        return Err(());
    }
    Ok((duration_secs / 4.0).ceil() as usize)
}

fn monitored_output_usage(root: &Path) -> Result<MonitoredOutputUsage, ()> {
    let mut usage = MonitoredOutputUsage {
        total_bytes: 0,
        entry_count: 0,
    };
    if !root.exists() {
        return Ok(usage);
    }
    for entry in fs::read_dir(root).map_err(|_| ())? {
        let entry = entry.map_err(|_| ())?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| ())?;
        usage.entry_count = usage.entry_count.checked_add(1).ok_or(())?;
        if metadata.file_type().is_symlink() {
            return Err(());
        }
        if metadata.is_dir() {
            let child = monitored_output_usage(&path)?;
            usage.total_bytes = usage.total_bytes.checked_add(child.total_bytes).ok_or(())?;
            usage.entry_count = usage.entry_count.checked_add(child.entry_count).ok_or(())?;
            continue;
        }
        if !metadata.is_file() {
            return Err(());
        }
        usage.total_bytes = usage.total_bytes.checked_add(metadata.len()).ok_or(())?;
    }
    Ok(usage)
}

fn validate_operation_id(value: &str) -> Result<(), ()> {
    if value.len() != OPERATION_ID_HEX_LEN
        || !value
            .as_bytes()
            .iter()
            .all(|byte| matches!(*byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(());
    }
    Ok(())
}

fn control_request_has_exact_fields(value: &Value, op: &str) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    match op {
        "init" => object.len() == 2 && object.contains_key("op") && object.contains_key("config"),
        "status" | "shutdown" => object.len() == 1 && object.contains_key("op"),
        _ => false,
    }
}

fn strip_runtime_invocation_envelope(
    value: &mut Value,
    expected_target: &str,
    expected_op: &str,
) -> Result<(), ()> {
    let object = value.as_object_mut().ok_or(())?;
    if object.contains_key("_runtime_transfer") {
        return Err(());
    }
    let envelope = object.remove("_runtime_invocation").ok_or(())?;
    let envelope = envelope.as_object().ok_or(())?;
    if envelope.len() != 10 {
        return Err(());
    }
    if envelope.get("schema").and_then(Value::as_str) != Some("elastos.provider.invocation/v1") {
        return Err(());
    }
    if envelope.get("source").and_then(Value::as_str) != Some("runtime")
        || envelope.get("target").and_then(Value::as_str) != Some(expected_target)
        || envelope.get("op").and_then(Value::as_str) != Some(expected_op)
        || envelope.get("capability").and_then(Value::as_str)
            != Some("provider:runtime->media:prepare")
        || envelope.get("transport").and_then(Value::as_str) != Some("runtime-local-provider-plane")
        || envelope.get("transfer").and_then(Value::as_str) != Some("json")
    {
        return Err(());
    }
    if !envelope.get("carrier").unwrap_or(&Value::Null).is_null()
        || !envelope.get("range").unwrap_or(&Value::Null).is_null()
        || !envelope.get("progress").unwrap_or(&Value::Null).is_null()
    {
        return Err(());
    }
    Ok(())
}

fn read_provider_frame<R: BufRead>(reader: &mut R) -> io::Result<Option<Result<Vec<u8>, ()>>> {
    let mut frame = Vec::new();
    let mut oversized = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if frame.is_empty() && !oversized {
                return Ok(None);
            }
            return Ok(Some(if oversized { Err(()) } else { Ok(frame) }));
        }
        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            let chunk = &available[..newline];
            if !oversized {
                if frame.len().saturating_add(chunk.len()) > MAX_PROVIDER_FRAME_BYTES {
                    oversized = true;
                } else {
                    frame.extend_from_slice(chunk);
                }
            }
            reader.consume(newline + 1);
            return Ok(Some(if oversized { Err(()) } else { Ok(frame) }));
        }
        let consumed = available.len();
        if !oversized {
            if frame.len().saturating_add(consumed) > MAX_PROVIDER_FRAME_BYTES {
                oversized = true;
            } else {
                frame.extend_from_slice(available);
            }
        }
        reader.consume(consumed);
    }
}

fn invalid_request() -> ProviderResponse {
    ProviderResponse::error(REQUEST_ERROR_CODE, "media provider request is invalid")
}

fn run_provider_loop<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    provider: &mut MediaProvider,
) {
    loop {
        let (response, should_exit) = match read_provider_frame(input) {
            Ok(Some(Ok(frame))) => provider.handle_frame(&frame),
            Ok(Some(Err(()))) => (invalid_request(), false),
            Ok(None) => break,
            Err(_) => break,
        };
        let bytes = match serde_json::to_vec(&response) {
            Ok(bytes) => bytes,
            Err(_) => break,
        };
        if output.write_all(&bytes).is_err()
            || output.write_all(b"\n").is_err()
            || output.flush().is_err()
        {
            break;
        }
        if should_exit {
            break;
        }
    }
}

pub fn run_provider_process() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut provider = MediaProvider::new();
    let mut input = stdin.lock();
    let mut writer = stdout.lock();
    run_provider_loop(&mut input, &mut writer, &mut provider);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn ffmpeg_timeout_reports_exact_spawn_and_reap_events() {
        let root = tempfile::tempdir().unwrap();
        let ffmpeg_path = root.path().join("ffmpeg");
        fs::write(&ffmpeg_path, "#!/bin/sh\nwhile :; do :; done\n").unwrap();
        fs::set_permissions(&ffmpeg_path, fs::Permissions::from_mode(0o700)).unwrap();
        let staging_root = root.path().join("staging");
        fs::create_dir(&staging_root).unwrap();
        fs::set_permissions(&staging_root, fs::Permissions::from_mode(0o700)).unwrap();
        let input_path = root.path().join("input.bin");
        fs::write(&input_path, b"video").unwrap();
        let prepared_dir = root.path().join("prepared");
        fs::create_dir(&prepared_dir).unwrap();
        fs::create_dir(prepared_dir.join("segments")).unwrap();
        let state = ConfiguredMediaProvider {
            staging_root,
            ffmpeg_path,
            ffprobe_path: root.path().join("unused-ffprobe"),
            timeout: Duration::ZERO,
            max_stdio_bytes: 1024,
            max_input_bytes: 1024,
            max_output_part_bytes: 1024,
            max_duration_secs: 1,
            max_source_width: 1,
            max_source_height: 1,
            max_source_fps: 1,
            max_segment_count: 1,
            max_total_output_bytes: 1024,
        };
        let mut events = Vec::new();

        assert!(
            run_ffmpeg_observed(&state, &input_path, &prepared_dir, &mut |event| {
                events.push(event)
            })
            .is_err()
        );

        let [CommandLifecycleEvent::Spawned { child_id: spawned }, CommandLifecycleEvent::Reaped {
            child_id: reaped,
            code,
            signal,
        }] = events.as_slice()
        else {
            panic!("unexpected FFmpeg child lifecycle: {events:?}");
        };
        assert_eq!(spawned, reaped);
        assert_eq!(*code, None);
        assert_eq!(*signal, Some(libc::SIGKILL));
    }

    #[test]
    fn rejects_noncanonical_operation_id() {
        assert!(validate_operation_id(&"a".repeat(64)).is_ok());
        assert!(validate_operation_id("").is_err());
        assert!(validate_operation_id("a").is_err());
        assert!(validate_operation_id(&"A".repeat(64)).is_err());
        assert!(validate_operation_id(&"g".repeat(64)).is_err());
        assert!(validate_operation_id(&"a".repeat(63)).is_err());
    }

    #[test]
    fn parse_frame_rate_falls_back_to_r_frame_rate_when_avg_is_invalid_or_zero() {
        assert_eq!(
            parse_frame_rate(Some("oops"), Some("30000/1001")).unwrap(),
            30000.0 / 1001.0
        );
        assert_eq!(
            parse_frame_rate(Some("0/1"), Some("30000/1001")).unwrap(),
            30000.0 / 1001.0
        );
    }

    #[test]
    fn parse_frame_rate_uses_r_frame_rate_when_avg_is_missing() {
        assert_eq!(parse_frame_rate(None, Some("24/1")).unwrap(), 24.0);
    }
}
