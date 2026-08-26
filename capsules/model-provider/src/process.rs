use crate::contract::{error_response, parse_request, ProviderFault};
use crate::execution::ProviderCoordinatorHandle;
use anyhow::Result;
use std::io::{BufRead, BufReader, Write};

const MAX_REQUEST_FRAME_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_FRAME_BYTES: usize = MAX_REQUEST_FRAME_BYTES;

pub fn run_main() {
    if let Err(err) = run_stdio() {
        eprintln!("[model-provider] fatal stdio failure: {err}");
        let response = error_response("provider_failed", "model provider failed");
        println!(
            "{}",
            serde_json::to_string(&response).unwrap_or_else(|_| {
                "{\"status\":\"error\",\"code\":\"provider_failed\",\"message\":\"model provider failed\"}"
                    .to_string()
            })
        );
        std::process::exit(1);
    }
}

fn run_stdio() -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    run_stdio_io(BufReader::new(stdin.lock()), stdout.lock())
}

fn run_stdio_io(mut reader: impl BufRead, mut writer: impl Write) -> Result<()> {
    let mut frame = Vec::new();
    let mut provider = ProviderCoordinatorHandle::start();

    loop {
        let Some(frame_status) = read_next_frame(&mut reader, &mut frame)? else {
            break;
        };
        let response = match frame_status {
            FrameStatus::Complete => match parse_frame(&frame) {
                Ok(line) => match parse_request(line) {
                    Ok(request) => provider
                        .request(request)
                        .map_err(|_| ProviderFault::internal("provider coordinator failed")),
                    Err(err) => Err(err),
                },
                Err(err) => Err(err),
            },
            FrameStatus::Oversized => Err(ProviderFault::invalid_frame(MAX_REQUEST_FRAME_BYTES)),
        };
        let response = match response {
            Ok(value) => value,
            Err(err) => {
                err.log();
                error_response(err.code(), err.message())
            }
        };
        write_response(&mut writer, &response)?;
        if response.get("data").and_then(|value| value.get("shutdown"))
            == Some(&serde_json::Value::Bool(true))
        {
            break;
        }
    }

    provider.shutdown_on_eof();

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameStatus {
    Complete,
    Oversized,
}

fn read_next_frame(
    reader: &mut impl BufRead,
    frame: &mut Vec<u8>,
) -> std::io::Result<Option<FrameStatus>> {
    frame.clear();
    let mut oversized = false;
    let frame_cap = MAX_REQUEST_FRAME_BYTES.saturating_add(1);

    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            if frame.is_empty() && !oversized {
                return Ok(None);
            }
            return Ok(Some(if oversized {
                FrameStatus::Oversized
            } else {
                FrameStatus::Complete
            }));
        }

        let newline_len = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1);
        let consume_len = newline_len.unwrap_or(buffer.len());

        if !oversized {
            let copy_len = frame_cap.saturating_sub(frame.len()).min(consume_len);
            frame.extend_from_slice(&buffer[..copy_len]);
            if frame.len() > MAX_REQUEST_FRAME_BYTES {
                oversized = true;
            }
        }

        reader.consume(consume_len);
        if newline_len.is_some() {
            return Ok(Some(if oversized {
                FrameStatus::Oversized
            } else {
                FrameStatus::Complete
            }));
        }
    }
}

fn parse_frame(frame: &[u8]) -> std::result::Result<&str, ProviderFault> {
    if frame.len() > MAX_REQUEST_FRAME_BYTES {
        return Err(ProviderFault::invalid_frame(MAX_REQUEST_FRAME_BYTES));
    }
    let line = std::str::from_utf8(frame)
        .map_err(|_| ProviderFault::invalid_request("provider request must be valid utf-8"))?;
    Ok(line.trim_end_matches(['\r', '\n']))
}

fn write_response(writer: &mut impl Write, response: &serde_json::Value) -> Result<()> {
    let frame = serialize_response_frame(response)?;
    writer.write_all(&frame)?;
    writer.flush()?;
    Ok(())
}

fn serialize_response_frame(response: &serde_json::Value) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(response)?;
    bytes.push(b'\n');
    if bytes.len() <= MAX_RESPONSE_FRAME_BYTES {
        return Ok(bytes);
    }
    let mut fallback = serde_json::to_vec(&error_response(
        "response_too_large",
        "model provider response exceeds frame limit",
    ))?;
    fallback.push(b'\n');
    if fallback.len() > MAX_RESPONSE_FRAME_BYTES {
        anyhow::bail!("model provider fallback response exceeds frame limit");
    }
    Ok(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::io::Cursor;

    #[test]
    fn frame_exact_limit_is_accepted() {
        let body = "a".repeat(MAX_REQUEST_FRAME_BYTES - 1);
        let mut reader = BufReader::new(Cursor::new(format!("{body}\n").into_bytes()));
        let mut frame = Vec::new();
        let status = read_next_frame(&mut reader, &mut frame).unwrap();
        assert_eq!(status, Some(FrameStatus::Complete));
        assert_eq!(frame.len(), MAX_REQUEST_FRAME_BYTES);
    }

    #[test]
    fn frame_limit_plus_one_is_rejected() {
        let body = "a".repeat(MAX_REQUEST_FRAME_BYTES);
        let mut reader = BufReader::new(Cursor::new(format!("{body}\n").into_bytes()));
        let mut frame = Vec::new();
        let status = read_next_frame(&mut reader, &mut frame).unwrap();
        assert_eq!(status, Some(FrameStatus::Oversized));
        assert_eq!(frame.len(), MAX_REQUEST_FRAME_BYTES + 1);
    }

    #[test]
    fn oversized_frame_followed_by_valid_frame_stays_synchronized() {
        let oversized = "a".repeat(MAX_REQUEST_FRAME_BYTES);
        let input = format!("{oversized}\n{{\"op\":\"status\"}}\n");
        let mut output = Vec::new();
        run_stdio_io(BufReader::new(Cursor::new(input.into_bytes())), &mut output).unwrap();
        let lines = decode_lines(&output);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["code"], "invalid_frame");
        assert_eq!(lines[1]["status"], "ok");
        assert_eq!(lines[1]["data"]["provider"], crate::contract::PROVIDER_ID);
    }

    #[test]
    fn final_eof_frame_is_processed() {
        let mut output = Vec::new();
        run_stdio_io(
            BufReader::new(Cursor::new(br#"{"op":"status"}"#.to_vec())),
            &mut output,
        )
        .unwrap();
        let lines = decode_lines(&output);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["status"], "ok");
        assert_eq!(lines[0]["data"]["provider"], crate::contract::PROVIDER_ID);
    }

    #[test]
    fn oversized_response_is_replaced_with_small_typed_error() {
        let response = serde_json::json!({
            "status": "ok",
            "data": {
                "payload": "x".repeat(MAX_RESPONSE_FRAME_BYTES)
            }
        });
        let frame = serialize_response_frame(&response).unwrap();
        assert!(frame.len() <= MAX_RESPONSE_FRAME_BYTES);
        let value: Value = serde_json::from_slice(&frame[..frame.len() - 1]).unwrap();
        assert_eq!(value["status"], "error");
        assert_eq!(value["code"], "response_too_large");
    }

    fn decode_lines(output: &[u8]) -> Vec<Value> {
        String::from_utf8(output.to_vec())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}
