#![cfg(unix)]

use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};

use elastos_protected_content_provider_contracts::ValidatedClearFmp4MediaSessionLayoutV1;
use serde_json::json;

const OPERATION_ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const NORMAL_TIMEOUT_MS: u64 = 5_000;
const SHORT_TIMEOUT_MS: u64 = 1_500;

#[derive(Clone, Copy)]
struct InitRequestConfig {
    timeout_ms: u64,
    max_stdio_bytes: usize,
    max_input_bytes: u64,
    max_output_part_bytes: u64,
    max_segment_count: usize,
    max_total_output_bytes: u64,
    max_duration_secs: u64,
    max_source_width: u32,
    max_source_height: u32,
    max_source_fps: u32,
}

impl Default for InitRequestConfig {
    fn default() -> Self {
        Self {
            timeout_ms: NORMAL_TIMEOUT_MS,
            max_stdio_bytes: 4096,
            max_input_bytes: 1 << 20,
            max_output_part_bytes: 1 << 20,
            max_segment_count: 8,
            max_total_output_bytes: 1 << 22,
            max_duration_secs: 60,
            max_source_width: 1920,
            max_source_height: 1080,
            max_source_fps: 60,
        }
    }
}

struct ProviderProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr: Option<ChildStderr>,
}

impl ProviderProcess {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_media-provider"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Self {
            stdin: Some(child.stdin.take().unwrap()),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            stderr: child.stderr.take(),
            child,
        }
    }

    fn request_json(&mut self, value: serde_json::Value) -> serde_json::Value {
        let stdin = self.stdin.as_mut().unwrap();
        serde_json::to_writer(&mut *stdin, &value).unwrap();
        writeln!(stdin).unwrap();
        stdin.flush().unwrap();
        self.read_response()
    }

    fn request_raw_line(&mut self, bytes: &[u8]) -> serde_json::Value {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(bytes).unwrap();
        writeln!(stdin).unwrap();
        stdin.flush().unwrap();
        self.read_response()
    }

    fn read_response(&mut self) -> serde_json::Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn shutdown_and_assert_clean(mut self) {
        let response = self.request_json(json!({"op":"shutdown"}));
        assert_eq!(response["status"], "ok");
        let status = self.child.wait().unwrap();
        assert!(status.success());
        self.assert_empty_stderr();
    }

    fn assert_empty_stderr(&mut self) {
        let mut stderr = String::new();
        self.stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
    }
}

struct ToolFixture {
    _root: tempfile::TempDir,
    staging_root: PathBuf,
    ffprobe_path: PathBuf,
    ffmpeg_path: PathBuf,
    ffprobe_args_log: PathBuf,
    ffmpeg_args_log: PathBuf,
    ffprobe_pid: PathBuf,
    ffmpeg_pid: PathBuf,
    ffprobe_stdout: PathBuf,
    ffprobe_stderr: PathBuf,
    ffprobe_mode: PathBuf,
    ffmpeg_mode: PathBuf,
    ffmpeg_stderr: PathBuf,
}

impl ToolFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let staging_root = root.path().join("staging");
        fs::create_dir(&staging_root).unwrap();
        fs::set_permissions(&staging_root, fs::Permissions::from_mode(0o700)).unwrap();
        let tools = root.path().join("tools");
        fs::create_dir(&tools).unwrap();
        fs::set_permissions(&tools, fs::Permissions::from_mode(0o700)).unwrap();

        let ffprobe_args_log = root.path().join("ffprobe.args");
        let ffmpeg_args_log = root.path().join("ffmpeg.args");
        let ffprobe_pid = root.path().join("ffprobe.pid");
        let ffmpeg_pid = root.path().join("ffmpeg.pid");
        let ffprobe_stdout = root.path().join("ffprobe.stdout.json");
        let ffprobe_stderr = root.path().join("ffprobe.stderr.txt");
        let ffprobe_mode = root.path().join("ffprobe.mode");
        let ffmpeg_mode = root.path().join("ffmpeg.mode");
        let ffmpeg_stderr = root.path().join("ffmpeg.stderr.txt");
        let init_fixture = root.path().join("init.mp4");
        let segment_zero_fixture = root.path().join("00000000.m4s");
        let segment_one_fixture = root.path().join("00000001.m4s");
        let segment_two_fixture = root.path().join("00000002.m4s");
        let oversize_segment_fixture = root.path().join("oversize.m4s");

        fs::write(&ffprobe_stdout, valid_ffprobe_json()).unwrap();
        fs::write(&ffprobe_stderr, "").unwrap();
        fs::write(&ffprobe_mode, "normal\n").unwrap();
        fs::write(&ffmpeg_mode, "success\n").unwrap();
        fs::write(&ffmpeg_stderr, "").unwrap();

        fs::write(&init_fixture, clear_init_segment()).unwrap();
        fs::write(&segment_zero_fixture, clear_segment(1, b"segment-0")).unwrap();
        fs::write(&segment_one_fixture, clear_segment(1, b"segment-1")).unwrap();
        fs::write(&segment_two_fixture, clear_segment(1, b"segment-2")).unwrap();
        fs::write(&oversize_segment_fixture, vec![0x55; 4096]).unwrap();

        let ffprobe_path = tools.join("ffprobe.sh");
        let ffmpeg_path = tools.join("ffmpeg.sh");
        fs::write(
            &ffprobe_path,
            format!(
                "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$$\" > '{pid}'\n: > '{args}'\nfor arg in \"$@\"; do\n  printf '%s\\n' \"$arg\" >> '{args}'\ndone\nmode=\"$(tr -d '\\n' < '{mode}')\"\ncat '{stdout}'\nif [ -s '{stderr}' ]; then\n  cat '{stderr}' >&2\nfi\nif [ \"$mode\" = 'hang_after_output' ]; then\n  while :; do sleep 1; done\nfi\n",
                pid = ffprobe_pid.display(),
                args = ffprobe_args_log.display(),
                stdout = ffprobe_stdout.display(),
                stderr = ffprobe_stderr.display(),
                mode = ffprobe_mode.display(),
            ),
        )
        .unwrap();
        fs::write(
            &ffmpeg_path,
            format!(
                "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$$\" > '{pid}'\n: > '{args}'\nmanifest=''\nfor arg in \"$@\"; do\n  manifest=\"$arg\"\n  printf '%s\\n' \"$arg\" >> '{args}'\ndone\nmode=\"$(tr -d '\\n' < '{mode}')\"\nif [ -s '{stderr}' ]; then\n  cat '{stderr}' >&2\nfi\nmkdir -p ./segments\ncase \"$mode\" in\n  success)\n    cp '{init_src}' ./init.mp4\n    cp '{seg0}' ./segments/00000000.m4s\n    cp '{seg1}' ./segments/00000001.m4s\n    printf '%s\\n' '<MPD />' > \"$manifest\"\n    ;;\n  too_many_segments)\n    cp '{init_src}' ./init.mp4\n    cp '{seg0}' ./segments/00000000.m4s\n    cp '{seg1}' ./segments/00000001.m4s\n    cp '{seg2}' ./segments/00000002.m4s\n    ;;\n  too_many_empty_segments_then_hang)\n    cp '{init_src}' ./init.mp4\n    : > ./segments/00000000.m4s\n    : > ./segments/00000001.m4s\n    : > ./segments/00000002.m4s\n    while :; do sleep 1; done\n    ;;\n  oversize_segment)\n    cp '{init_src}' ./init.mp4\n    cp '{oversize}' ./segments/00000000.m4s\n    ;;\n  oversize_then_hang)\n    cp '{init_src}' ./init.mp4\n    cp '{oversize}' ./segments/00000000.m4s\n    while :; do sleep 1; done\n    ;;\n  exit_nonzero)\n    exit 17\n    ;;\n  timeout)\n    while :; do sleep 1; done\n    ;;\n  *)\n    exit 23\n    ;;\nesac\n",
                pid = ffmpeg_pid.display(),
                args = ffmpeg_args_log.display(),
                mode = ffmpeg_mode.display(),
                stderr = ffmpeg_stderr.display(),
                init_src = init_fixture.display(),
                seg0 = segment_zero_fixture.display(),
                seg1 = segment_one_fixture.display(),
                seg2 = segment_two_fixture.display(),
                oversize = oversize_segment_fixture.display(),
            ),
        )
        .unwrap();
        fs::set_permissions(&ffprobe_path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&ffmpeg_path, fs::Permissions::from_mode(0o700)).unwrap();

        Self {
            _root: root,
            staging_root,
            ffprobe_path,
            ffmpeg_path,
            ffprobe_args_log,
            ffmpeg_args_log,
            ffprobe_pid,
            ffmpeg_pid,
            ffprobe_stdout,
            ffprobe_stderr,
            ffprobe_mode,
            ffmpeg_mode,
            ffmpeg_stderr,
        }
    }

    fn init_request(&self, config: InitRequestConfig) -> serde_json::Value {
        json!({
            "op": "init",
            "config": {
                "base_path": "",
                "allowed_paths": [],
                "read_only": false,
                "encryption_key": "",
                "extra": {
                    "provider_id": "media-provider",
                    "staging_root": self.staging_root,
                    "ffmpeg_path": self.ffmpeg_path,
                    "ffprobe_path": self.ffprobe_path,
                    "output_profile": "browser_fmp4_h264_v1",
                    "timeout_ms": config.timeout_ms,
                    "max_stdio_bytes": config.max_stdio_bytes,
                    "max_input_bytes": config.max_input_bytes,
                    "max_output_part_bytes": config.max_output_part_bytes,
                    "max_duration_secs": config.max_duration_secs,
                    "max_source_width": config.max_source_width,
                    "max_source_height": config.max_source_height,
                    "max_source_fps": config.max_source_fps,
                    "max_segment_count": config.max_segment_count,
                    "max_total_output_bytes": config.max_total_output_bytes
                }
            }
        })
    }

    fn set_ffprobe_output(&self, output: &str) {
        fs::write(&self.ffprobe_stdout, output).unwrap();
    }

    fn set_ffprobe_stderr(&self, output: &str) {
        fs::write(&self.ffprobe_stderr, output).unwrap();
    }

    fn set_ffprobe_mode(&self, mode: &str) {
        fs::write(&self.ffprobe_mode, format!("{mode}\n")).unwrap();
    }

    fn set_ffmpeg_mode(&self, mode: &str) {
        fs::write(&self.ffmpeg_mode, format!("{mode}\n")).unwrap();
    }

    fn set_ffmpeg_stderr(&self, output: &str) {
        fs::write(&self.ffmpeg_stderr, output).unwrap();
    }

    fn prepare_operation(&self) -> PathBuf {
        let root = self.staging_root.join(OPERATION_ID);
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let input = root.join("input.bin");
        fs::write(&input, b"source-video").unwrap();
        fs::set_permissions(&input, fs::Permissions::from_mode(0o600)).unwrap();
        root
    }
}

fn valid_ffprobe_json() -> &'static str {
    r#"{"streams":[{"codec_type":"video","width":1280,"height":720,"avg_frame_rate":"30000/1001","r_frame_rate":"30000/1001"}],"format":{"duration":"4.25"}}"#
}

fn runtime_prepare_request() -> serde_json::Value {
    json!({
        "op": "prepare",
        "operation_id": OPERATION_ID,
        "_runtime_invocation": {
            "schema": "elastos.provider.invocation/v1",
            "source": "runtime",
            "target": "media",
            "op": "prepare",
            "capability": "provider:runtime->media:prepare",
            "transport": "runtime-local-provider-plane",
            "carrier": null,
            "transfer": "json",
            "range": null,
            "progress": null
        }
    })
}

fn read_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(ToString::to_string)
        .collect()
}

fn read_pid(path: &Path) -> u32 {
    fs::read_to_string(path).unwrap().trim().parse().unwrap()
}

fn read_pid_within(path: &Path) -> u32 {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        if path.exists() {
            return read_pid(path);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pid file missing: {}",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn process_exists(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn make_box(kind: &[u8; 4], content: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + content.len());
    out.extend_from_slice(&(u32::try_from(8 + content.len()).unwrap()).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(content);
    out
}

fn make_fullbox(kind: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut content = Vec::with_capacity(4 + payload.len());
    content.push(version);
    content.extend_from_slice(&flags.to_be_bytes()[1..]);
    content.extend_from_slice(payload);
    make_box(kind, &content)
}

fn make_clear_track(track_id: u32, handler_type: &[u8; 4]) -> Vec<u8> {
    let mut tkhd_payload = vec![0u8; 12];
    tkhd_payload[8..12].copy_from_slice(&track_id.to_be_bytes());
    let tkhd = make_fullbox(b"tkhd", 0, 0, &tkhd_payload);
    let mut hdlr_payload = vec![0u8; 4];
    hdlr_payload.extend_from_slice(handler_type);
    let hdlr = make_fullbox(b"hdlr", 0, 0, &hdlr_payload);
    let (entry_type, fixed) = match handler_type {
        b"vide" => (b"avc1", 78usize),
        b"soun" => (b"mp4a", 28usize),
        _ => panic!("unsupported handler"),
    };
    let entry = make_box(entry_type, &vec![0u8; fixed]);
    let mut stsd_payload = vec![0u8; 4];
    stsd_payload.extend_from_slice(&1u32.to_be_bytes());
    stsd_payload.extend_from_slice(&entry);
    let stsd = make_box(b"stsd", &stsd_payload);
    let stbl = make_box(b"stbl", &stsd);
    let minf = make_box(b"minf", &stbl);
    let mut mdia_content = Vec::new();
    mdia_content.extend_from_slice(&hdlr);
    mdia_content.extend_from_slice(&minf);
    let mdia = make_box(b"mdia", &mdia_content);
    let mut trak_content = Vec::new();
    trak_content.extend_from_slice(&tkhd);
    trak_content.extend_from_slice(&mdia);
    make_box(b"trak", &trak_content)
}

fn clear_init_segment() -> Vec<u8> {
    let ftyp = make_box(b"ftyp", b"isom\0\0\0\0isomiso6");
    let trak_video = make_clear_track(1, b"vide");
    let trex_video = make_fullbox(
        b"trex",
        0,
        0,
        &[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    );
    let mvex = make_box(b"mvex", &trex_video);
    let mvhd = make_box(b"mvhd", &[0u8; 4]);
    let moov = {
        let mut content = Vec::new();
        content.extend_from_slice(&mvhd);
        content.extend_from_slice(&trak_video);
        content.extend_from_slice(&mvex);
        make_box(b"moov", &content)
    };
    [ftyp, moov].concat()
}

fn clear_segment(track_id: u32, payload: &[u8]) -> Vec<u8> {
    const TFHD_FLAGS_PRODUCER_V1: u32 = 0x020038;
    const TRUN_FLAG_DATA_OFFSET: u32 = 0x000001;
    const TRUN_FLAG_SAMPLE_SIZE: u32 = 0x000200;

    let mfhd = make_fullbox(b"mfhd", 0, 0, &1u32.to_be_bytes());
    let tfhd = {
        let mut payload_bytes = Vec::new();
        payload_bytes.extend_from_slice(&track_id.to_be_bytes());
        payload_bytes.extend_from_slice(&1u32.to_be_bytes());
        payload_bytes.extend_from_slice(&(u32::try_from(payload.len()).unwrap()).to_be_bytes());
        payload_bytes.extend_from_slice(&0u32.to_be_bytes());
        make_fullbox(b"tfhd", 0, TFHD_FLAGS_PRODUCER_V1, &payload_bytes)
    };
    let tfdt = make_fullbox(b"tfdt", 0, 0, &1u32.to_be_bytes());
    let trun = {
        let mut payload_bytes = Vec::new();
        payload_bytes.extend_from_slice(&1u32.to_be_bytes());
        payload_bytes.extend_from_slice(&0u32.to_be_bytes());
        payload_bytes.extend_from_slice(&(u32::try_from(payload.len()).unwrap()).to_be_bytes());
        make_fullbox(
            b"trun",
            0,
            TRUN_FLAG_DATA_OFFSET | TRUN_FLAG_SAMPLE_SIZE,
            &payload_bytes,
        )
    };
    let traf = {
        let mut content = Vec::new();
        content.extend_from_slice(&tfhd);
        content.extend_from_slice(&tfdt);
        content.extend_from_slice(&trun);
        make_box(b"traf", &content)
    };
    let mut moof = {
        let mut content = Vec::new();
        content.extend_from_slice(&mfhd);
        content.extend_from_slice(&traf);
        make_box(b"moof", &content)
    };
    let data_offset = (moof.len() + 8) as i32;
    let trun_offset = moof
        .windows(4)
        .position(|window| window == b"trun")
        .expect("trun box present")
        - 4;
    let trun_data_offset_at = trun_offset + 16;
    moof[trun_data_offset_at..trun_data_offset_at + 4].copy_from_slice(&data_offset.to_be_bytes());
    let mdat = make_box(b"mdat", payload);
    [moof, mdat].concat()
}

#[test]
fn process_init_status_prepare_shutdown_emits_valid_clear_fmp4() {
    let fixture = ToolFixture::new();
    let operation_root = fixture.prepare_operation();
    let fixture_session =
        ValidatedClearFmp4MediaSessionLayoutV1::new(&clear_init_segment()).unwrap();
    fixture_session
        .validate_segment(&clear_segment(1, b"segment-0"))
        .unwrap();
    let mut provider = ProviderProcess::start();

    let init = provider.request_json(fixture.init_request(InitRequestConfig::default()));
    assert_eq!(init["status"], "ok");
    assert_eq!(init["data"]["configured"], true);

    let status = provider.request_json(json!({"op":"status"}));
    assert_eq!(status["status"], "ok");
    assert_eq!(status["data"]["provider"], "media-provider");

    let prepare = provider.request_json(runtime_prepare_request());
    assert_eq!(prepare["status"], "ok");
    assert_eq!(
        prepare["data"],
        json!({
            "schema": "elastos.media-provider.prepared-media/v1",
            "mime_type": "video/mp4",
            "codecs": "avc1.640028"
        })
    );

    let ffprobe_args = read_lines(&fixture.ffprobe_args_log);
    assert_eq!(
        ffprobe_args,
        vec![
            "-v".to_string(),
            "error".to_string(),
            "-print_format".to_string(),
            "json".to_string(),
            "-show_entries".to_string(),
            "format=duration:stream=codec_type,width,height,avg_frame_rate,r_frame_rate"
                .to_string(),
            operation_root
                .join("input.bin")
                .to_string_lossy()
                .to_string(),
        ]
    );
    let ffmpeg_args = read_lines(&fixture.ffmpeg_args_log);
    assert_eq!(
        ffmpeg_args,
        vec![
            "-nostdin".to_string(),
            "-v".to_string(),
            "error".to_string(),
            "-y".to_string(),
            "-i".to_string(),
            operation_root.join("input.bin").to_string_lossy().to_string(),
            "-map".to_string(),
            "0:v:0".to_string(),
            "-an".to_string(),
            "-c:v".to_string(),
            "libx264".to_string(),
            "-profile:v".to_string(),
            "high".to_string(),
            "-level:v".to_string(),
            "4.0".to_string(),
            "-pix_fmt".to_string(),
            "yuv420p".to_string(),
            "-r".to_string(),
            "30".to_string(),
            "-vf".to_string(),
            "scale=w=min(iw\\,1920):h=min(ih\\,1080):force_original_aspect_ratio=decrease:force_divisible_by=2,fps=30".to_string(),
            "-g".to_string(),
            "120".to_string(),
            "-keyint_min".to_string(),
            "120".to_string(),
            "-sc_threshold".to_string(),
            "0".to_string(),
            "-preset".to_string(),
            "veryfast".to_string(),
            "-crf".to_string(),
            "28".to_string(),
            "-movflags".to_string(),
            "+frag_keyframe+empty_moov+default_base_moof+separate_moof".to_string(),
            "-f".to_string(),
            "dash".to_string(),
            "-seg_duration".to_string(),
            "4".to_string(),
            "-streaming".to_string(),
            "1".to_string(),
            "-use_timeline".to_string(),
            "0".to_string(),
            "-use_template".to_string(),
            "1".to_string(),
            "-start_number".to_string(),
            "0".to_string(),
            "-init_seg_name".to_string(),
            "init.mp4".to_string(),
            "-media_seg_name".to_string(),
            "segments/$Number%08d$.m4s".to_string(),
            operation_root
                .join("prepared")
                .join("manifest.mpd")
                .to_string_lossy()
                .to_string(),
        ]
    );
    assert!(!operation_root.join("prepared/manifest.mpd").exists());
    let mut prepared_entries = fs::read_dir(operation_root.join("prepared"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    prepared_entries.sort();
    assert_eq!(
        prepared_entries,
        vec!["init.mp4".to_string(), "segments".to_string()]
    );

    let init_bytes = fs::read(operation_root.join("prepared/init.mp4")).unwrap();
    let segment_zero = fs::read(operation_root.join("prepared/segments/00000000.m4s")).unwrap();
    let segment_one = fs::read(operation_root.join("prepared/segments/00000001.m4s")).unwrap();
    let session = ValidatedClearFmp4MediaSessionLayoutV1::new(&init_bytes).unwrap();
    session.validate_segment(&segment_zero).unwrap();
    session.validate_segment(&segment_one).unwrap();

    provider.shutdown_and_assert_clean();
}

#[test]
fn process_timeout_returns_settled_error_and_provider_remains_available() {
    let fixture = ToolFixture::new();
    fixture.set_ffmpeg_mode("timeout");
    fixture.prepare_operation();
    let mut provider = ProviderProcess::start();
    let init = provider.request_json(fixture.init_request(InitRequestConfig {
        timeout_ms: SHORT_TIMEOUT_MS,
        ..InitRequestConfig::default()
    }));
    assert_eq!(init["status"], "ok");

    let prepare = provider.request_json(runtime_prepare_request());
    assert_eq!(prepare["status"], "error");
    assert_eq!(prepare["code"], "internal_error");
    assert_eq!(prepare["data"]["operation_settled"], true);

    provider.shutdown_and_assert_clean();
}

#[test]
fn process_prepare_rejects_nonzero_ffmpeg_exit() {
    let fixture = ToolFixture::new();
    fixture.set_ffmpeg_mode("exit_nonzero");
    fixture.prepare_operation();
    let mut provider = ProviderProcess::start();
    let init = provider.request_json(fixture.init_request(InitRequestConfig::default()));
    assert_eq!(init["status"], "ok");

    let prepare = provider.request_json(runtime_prepare_request());
    assert_eq!(prepare["status"], "error");
    assert_eq!(prepare["code"], "internal_error");

    provider.shutdown_and_assert_clean();
}

#[test]
fn process_prepare_rejects_malformed_probe_output() {
    let fixture = ToolFixture::new();
    fixture.set_ffprobe_output("{not-json}\n");
    fixture.prepare_operation();
    let mut provider = ProviderProcess::start();
    let init = provider.request_json(fixture.init_request(InitRequestConfig::default()));
    assert_eq!(init["status"], "ok");

    let prepare = provider.request_json(runtime_prepare_request());
    assert_eq!(prepare["status"], "error");
    assert_eq!(prepare["code"], "internal_error");

    provider.shutdown_and_assert_clean();
}

#[test]
fn process_prepare_rejects_probe_without_video() {
    let fixture = ToolFixture::new();
    fixture
        .set_ffprobe_output(r#"{"streams":[{"codec_type":"audio"}],"format":{"duration":"4.25"}}"#);
    fixture.prepare_operation();
    let mut provider = ProviderProcess::start();
    let init = provider.request_json(fixture.init_request(InitRequestConfig::default()));
    assert_eq!(init["status"], "ok");

    let prepare = provider.request_json(runtime_prepare_request());
    assert_eq!(prepare["status"], "error");
    assert_eq!(prepare["code"], "internal_error");

    provider.shutdown_and_assert_clean();
}

#[test]
fn process_prepare_rejects_nonfinite_duration_and_dimension_limit_violations() {
    let fixture = ToolFixture::new();
    fixture.prepare_operation();
    let mut provider = ProviderProcess::start();
    let init = provider.request_json(fixture.init_request(InitRequestConfig::default()));
    assert_eq!(init["status"], "ok");

    fixture.set_ffprobe_output(
        r#"{"streams":[{"codec_type":"video","width":1280,"height":720,"avg_frame_rate":"30000/1001","r_frame_rate":"30000/1001"}],"format":{"duration":"NaN"}}"#,
    );
    let prepare = provider.request_json(runtime_prepare_request());
    assert_eq!(prepare["status"], "error");

    fixture.set_ffprobe_output(
        r#"{"streams":[{"codec_type":"video","width":1921,"height":720,"avg_frame_rate":"30000/1001","r_frame_rate":"30000/1001"}],"format":{"duration":"4.25"}}"#,
    );
    let prepare = provider.request_json(runtime_prepare_request());
    assert_eq!(prepare["status"], "error");

    fixture.set_ffprobe_output(valid_ffprobe_json());
    let prepare = provider.request_json(runtime_prepare_request());
    assert_eq!(prepare["status"], "ok");

    provider.shutdown_and_assert_clean();
}

#[test]
fn process_prepare_enforces_segment_count_precheck_and_output_size_limits() {
    let fixture = ToolFixture::new();
    fixture.prepare_operation();
    let mut provider = ProviderProcess::start();
    let init = provider.request_json(fixture.init_request(InitRequestConfig {
        max_segment_count: 1,
        ..InitRequestConfig::default()
    }));
    assert_eq!(init["status"], "ok");

    fixture.set_ffprobe_output(
        r#"{"streams":[{"codec_type":"video","width":1280,"height":720,"avg_frame_rate":"30000/1001","r_frame_rate":"30000/1001"}],"format":{"duration":"8.25"}}"#,
    );
    let prepare = provider.request_json(runtime_prepare_request());
    assert_eq!(prepare["status"], "error");
    assert!(!fixture.ffmpeg_args_log.exists());

    fixture.set_ffprobe_output(valid_ffprobe_json());
    fixture.set_ffmpeg_mode("oversize_then_hang");
    let init = provider.request_json(fixture.init_request(InitRequestConfig {
        max_output_part_bytes: 64,
        max_total_output_bytes: 128,
        ..InitRequestConfig::default()
    }));
    assert_eq!(init["status"], "ok");
    let prepare = provider.request_json(runtime_prepare_request());
    assert_eq!(prepare["status"], "error");
    assert!(!process_exists(read_pid_within(&fixture.ffmpeg_pid)));

    fixture.set_ffmpeg_mode("too_many_empty_segments_then_hang");
    let prepare = provider.request_json(runtime_prepare_request());
    assert_eq!(prepare["status"], "error");
    assert!(!process_exists(read_pid_within(&fixture.ffmpeg_pid)));

    provider.shutdown_and_assert_clean();
}

#[test]
fn process_prepare_rejects_source_fps_over_operator_limit() {
    let fixture = ToolFixture::new();
    fixture.set_ffprobe_output(
        r#"{"streams":[{"codec_type":"video","width":1280,"height":720,"avg_frame_rate":"60000/1001","r_frame_rate":"60000/1001"}],"format":{"duration":"4.25"}}"#,
    );
    fixture.prepare_operation();
    let mut provider = ProviderProcess::start();
    let init = provider.request_json(fixture.init_request(InitRequestConfig {
        max_source_fps: 30,
        ..InitRequestConfig::default()
    }));
    assert_eq!(init["status"], "ok");

    let prepare = provider.request_json(runtime_prepare_request());
    assert_eq!(prepare["status"], "error");

    provider.shutdown_and_assert_clean();
}

#[test]
fn process_prepare_kills_tool_on_oversized_stdout_or_stderr() {
    let fixture = ToolFixture::new();
    fixture.prepare_operation();
    let mut provider = ProviderProcess::start();

    let oversized = "x".repeat(8192);
    fixture.set_ffprobe_output(&oversized);
    fixture.set_ffprobe_mode("hang_after_output");
    let init = provider.request_json(fixture.init_request(InitRequestConfig::default()));
    assert_eq!(init["status"], "ok");
    let prepare = provider.request_json(runtime_prepare_request());
    assert_eq!(prepare["status"], "error");
    assert!(!process_exists(read_pid_within(&fixture.ffprobe_pid)));

    fixture.set_ffprobe_output(valid_ffprobe_json());
    fixture.set_ffprobe_stderr("");
    fixture.set_ffprobe_mode("normal");
    fixture.set_ffmpeg_stderr(&oversized);
    fixture.set_ffmpeg_mode("timeout");
    let prepare = provider.request_json(runtime_prepare_request());
    assert_eq!(prepare["status"], "error");
    assert!(!process_exists(read_pid_within(&fixture.ffmpeg_pid)));

    provider.shutdown_and_assert_clean();
}

#[test]
fn process_rejects_oversized_request_frame_and_stays_synchronized() {
    let fixture = ToolFixture::new();
    let mut provider = ProviderProcess::start();
    let init = provider.request_json(fixture.init_request(InitRequestConfig::default()));
    assert_eq!(init["status"], "ok");

    let response = provider.request_raw_line(&vec![b'a'; 20 * 1024]);
    assert_eq!(response["status"], "error");
    assert_eq!(response["code"], "invalid_request");

    let status = provider.request_json(json!({"op":"status"}));
    assert_eq!(status["status"], "ok");
    assert_eq!(status["data"]["provider"], "media-provider");

    provider.shutdown_and_assert_clean();
}

#[test]
fn process_init_rejects_values_above_provider_hard_limits() {
    let fixture = ToolFixture::new();
    for request in [
        fixture.init_request(InitRequestConfig {
            timeout_ms: 3_600_001,
            ..InitRequestConfig::default()
        }),
        fixture.init_request(InitRequestConfig {
            max_stdio_bytes: (1 << 20) + 1,
            ..InitRequestConfig::default()
        }),
        fixture.init_request(InitRequestConfig {
            max_input_bytes: (1 << 30) + 1,
            ..InitRequestConfig::default()
        }),
    ] {
        let mut provider = ProviderProcess::start();
        let response = provider.request_json(request);
        assert_eq!(response["status"], "error");
        assert_eq!(response["code"], "invalid_config");
        provider.shutdown_and_assert_clean();
    }
}

#[test]
fn process_prepare_rejects_path_indirection_and_redacts_private_details() {
    let fixture = ToolFixture::new();
    let operation_root = fixture.prepare_operation();
    let input_path = operation_root.join("input.bin");
    let outside_path = fixture
        .staging_root
        .parent()
        .unwrap()
        .join("private-source-video");
    fs::write(&outside_path, b"private-video").unwrap();
    fs::remove_file(&input_path).unwrap();
    std::os::unix::fs::symlink(&outside_path, &input_path).unwrap();
    let mut provider = ProviderProcess::start();
    assert_eq!(
        provider.request_json(fixture.init_request(InitRequestConfig::default()))["status"],
        "ok"
    );

    let response = provider.request_json(runtime_prepare_request());

    assert_eq!(response["status"], "error");
    assert_eq!(response["code"], "internal_error");
    let encoded = serde_json::to_string(&response).unwrap();
    assert!(!encoded.contains(outside_path.to_string_lossy().as_ref()));
    assert!(!fixture.ffprobe_args_log.exists());
    provider.shutdown_and_assert_clean();
}

#[test]
fn process_prepare_rejects_nonruntime_authority_and_topology_fields() {
    let fixture = ToolFixture::new();
    fixture.prepare_operation();
    let mut provider = ProviderProcess::start();
    assert_eq!(
        provider.request_json(fixture.init_request(InitRequestConfig::default()))["status"],
        "ok"
    );
    let mut wrong_transport = runtime_prepare_request();
    wrong_transport["_runtime_invocation"]["transport"] = json!("carrier-provider-plane");
    wrong_transport["_runtime_invocation"]["carrier"] = json!({"peer_did":"did:example:peer"});
    let mut wrong_capability = runtime_prepare_request();
    wrong_capability["_runtime_invocation"]["capability"] = json!("provider:caller->media:prepare");
    let mut caller_path = runtime_prepare_request();
    caller_path["local_path"] = json!("/private/source.mp4");

    for request in [wrong_transport, wrong_capability, caller_path] {
        let response = provider.request_json(request);
        assert_eq!(response["status"], "error");
        assert_eq!(response["code"], "invalid_request");
    }
    assert!(!fixture.ffprobe_args_log.exists());
    provider.shutdown_and_assert_clean();
}

#[test]
fn process_timeout_after_caller_cancellation_reports_settlement_and_exits_cleanly() {
    let fixture = ToolFixture::new();
    fixture.set_ffmpeg_mode("timeout");
    fixture.prepare_operation();
    let mut provider = ProviderProcess::start();
    assert_eq!(
        provider.request_json(fixture.init_request(InitRequestConfig {
            timeout_ms: SHORT_TIMEOUT_MS,
            ..InitRequestConfig::default()
        }))["status"],
        "ok"
    );
    let mut stdin = provider.stdin.take().unwrap();
    serde_json::to_writer(&mut stdin, &runtime_prepare_request()).unwrap();
    writeln!(stdin).unwrap();
    stdin.flush().unwrap();
    drop(stdin);

    let response = provider.read_response();
    assert_eq!(response["status"], "error");
    assert_eq!(response["code"], "internal_error");
    assert_eq!(response["data"]["operation_settled"], true);
    assert!(provider.child.wait().unwrap().success());
    provider.assert_empty_stderr();
}

#[test]
fn process_prepare_accepts_extra_ffprobe_fields_and_falls_back_to_r_frame_rate() {
    let fixture = ToolFixture::new();
    fixture.set_ffprobe_output(
        r#"{"streams":[{"codec_type":"video","width":1280,"height":720,"avg_frame_rate":"oops","r_frame_rate":"30000/1001","codec_name":"h264","disposition":{"default":1}}],"format":{"duration":"4.25","format_name":"mov,mp4"},"packets_and_frames":{"ignored":true}}"#,
    );
    fixture.prepare_operation();
    let mut provider = ProviderProcess::start();
    let init = provider.request_json(fixture.init_request(InitRequestConfig::default()));
    assert_eq!(init["status"], "ok");

    let prepare = provider.request_json(runtime_prepare_request());
    assert_eq!(prepare["status"], "ok");

    provider.shutdown_and_assert_clean();
}
