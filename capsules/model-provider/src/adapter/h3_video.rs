//! h3_video adapter: video runs against the MiniMax H3 Generate 2× upstream
//! (Sparks LAN via tunnel). Multipart POST to `/v1/videos/sync`, mp4 bytes
//! streamed back on completion.
//!
//! Cancel honesty: the sync API has no remote abort. Cancel detaches the
//! client immediately and discards bytes when they arrive; the cluster may
//! finish the render server-side. Same semantics as the dogfood gateway.
//!
//! Output convention mirrors the dogfood Studio library: `<id>.mp4` +
//! `<id>.json` sidecar (`{id,status,mode,scale,prompt,duration}`) under
//! `<output_dir>` so clips are portable into the Home library by directory.

use crate::run::{ObjectDescriptor, Run, RunEvent, RunState, VideoParams};
use sha2::Digest;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/* No wall-clock ceiling: a run ends on completion, caller cancel (Stop), or genuine
   upstream failure — the caller owns the compute. */
const READ_CHUNK: usize = 256 * 1024;

fn multipart_body(fields: &[(&str, String)]) -> (Vec<u8>, String) {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let boundary = format!("modelprovider{nanos:x}");
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (body, boundary)
}

pub fn run_video(run: Arc<Mutex<Run>>, upstream_url: String, output_dir: PathBuf, params: VideoParams) {
    let (run_id, cancel) = {
        let guard = run.lock().unwrap();
        (
            guard.run_id.clone().replace(':', "-"),
            Arc::clone(&guard.cancel),
        )
    };
    let push = |event: RunEvent| run.lock().unwrap().push(event);
    let fail = |code: &str, message: &str| {
        push(RunEvent::Error {
            code: code.to_string(),
            message: message.to_string(),
        });
        push(RunEvent::State {
            state: RunState::Failed,
        });
    };

    push(RunState::Preparing.into());
    push(RunEvent::Progress {
        completed: 3,
        total: 100,
        phase: "starting".to_string(),
    });

    let _ = run_id; // run-scoped logging only; the artifact is content-addressed below
    if let Err(err) = std::fs::create_dir_all(&output_dir) {
        fail("output_dir", &err.to_string());
        return;
    }
    // Stream to a .partial file; the artifact id (sha256[..32]) is only known
    // once hashing completes, then we rename atomically.
    let partial_path = output_dir.join(format!("{run_id}.mp4.partial"));

    let extra = serde_json::json!({
        "task": "t2va",
        "duration": params.duration_seconds,
        "audio_flow_shift": 3.0,
    })
    .to_string();
    let fields: Vec<(&str, String)> = vec![
        ("prompt", params.prompt.clone()),
        ("width", "768".to_string()),
        ("height", "448".to_string()),
        ("fps", "24".to_string()),
        ("num_inference_steps", "20".to_string()),
        ("flow_shift", "12".to_string()),
        ("seed", "42".to_string()),
        ("extra_params", extra),
    ];
    let (body, boundary) = multipart_body(&fields);

    if cancel.load(Ordering::Relaxed) {
        push(RunState::Cancelled.into());
        return;
    }

    push(RunState::Running.into());
    push(RunEvent::Progress {
        completed: 10,
        total: 100,
        phase: format!("generating · {}s clip", params.duration_seconds),
    });

    let response = match ureq::post(&upstream_url)
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .send_bytes(&body)
    {
        Ok(response) => response,
        Err(err) => {
            fail("upstream_error", &err.to_string());
            return;
        }
    };

    push(RunEvent::Progress {
        completed: 90,
        total: 100,
        phase: "saving".to_string(),
    });

    let mut file = match std::fs::File::create(&partial_path) {
        Ok(file) => file,
        Err(err) => {
            fail("output_write", &err.to_string());
            return;
        }
    };
    let mut reader = response.into_reader();
    let mut hasher = sha2::Sha256::new();
    let mut size: u64 = 0;
    let mut buffer = [0u8; READ_CHUNK];

    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            let _ = std::fs::remove_file(&partial_path);
            push(RunState::Cancelled.into());
            return;
        }
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&buffer[..n]);
                size += n as u64;
                if let Err(err) = file.write_all(&buffer[..n]) {
                    fail("output_write", &err.to_string());
                    let _ = std::fs::remove_file(&partial_path);
                    return;
                }
            }
            Err(err) => {
                fail("stream_error", &err.to_string());
                let _ = std::fs::remove_file(&partial_path);
                return;
            }
        }
    }
    drop(file);

    if size == 0 {
        let _ = std::fs::remove_file(&partial_path);
        fail("empty_result", "upstream returned no video bytes");
        return;
    }

    let sha256 = format!("{:x}", hasher.finalize());
    // Artifact id = first 32 hex of the content hash: deterministic, content-
    // addressed, and matches the creative library's `is_creative_job_id`
    // (32-hex) shape so library + video endpoints resolve it with zero changes.
    let artifact_id = sha256[..32].to_string();
    let out_path = output_dir.join(format!("{artifact_id}.mp4"));
    if let Err(err) = std::fs::rename(&partial_path, &out_path) {
        fail("output_write", &err.to_string());
        let _ = std::fs::remove_file(&partial_path);
        return;
    }

    let sidecar = serde_json::json!({
        "id": artifact_id,
        "status": "done",
        "mode": "generate",
        "scale": 2,
        "prompt": params.prompt,
        "duration": params.duration_seconds,
        "sha256": sha256,
        "size": size,
    });
    let _ = std::fs::write(
        output_dir.join(format!("{artifact_id}.json")),
        sidecar.to_string(),
    );

    push(RunEvent::Result {
        objects: vec![ObjectDescriptor {
            id: artifact_id,
            media_type: "video/mp4".to_string(),
            sha256,
            size,
        }],
    });
    push(RunState::Succeeded.into());
}
