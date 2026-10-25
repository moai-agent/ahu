//! Minimal authoring example. Its single probe operation is entirely synthetic.
use ahu_harness_api::{MAX_FRAME_BYTES, Operation, PROTOCOL_VERSION, Request, Success};
use std::io::{BufRead, Read, Write};

fn serve() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().skip(1).collect::<Vec<_>>() != ["--ahu-harness-runtime-protocol=1"] {
        return Err("unsupported invocation".into());
    }
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut id = 1_u64;
    loop {
        let mut frame = Vec::new();
        // read_until alone can allocate an arbitrarily long line.
        let read = std::io::Read::by_ref(&mut input)
            .take(MAX_FRAME_BYTES as u64 + 1)
            .read_until(b'\n', &mut frame)?;
        if read == 0 {
            return Ok(());
        }
        if frame.len() > MAX_FRAME_BYTES || frame.last() != Some(&b'\n') {
            return Err("invalid frame".into());
        }
        let request: Request = ahu_harness_api::json::from_slice(&frame)?;
        if request.protocol_version != PROTOCOL_VERSION || request.id != id {
            return Err("invalid sequence".into());
        }
        if request.operation != Operation::Probe {
            return Err("unsupported operation".into());
        }
        // Each consumer needs its own strict payload type and validation.
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Probe {}
        let _: Probe = serde_json::from_value(request.params)?;
        serde_json::to_writer(
            &mut output,
            &Success {
                protocol_version: PROTOCOL_VERSION,
                id,
                result: serde_json::json!({"available":true}),
            },
        )?;
        output.write_all(b"\n")?;
        output.flush()?;
        id = id.checked_add(1).ok_or("sequence exhausted")?;
    }
}
fn main() {
    if serve().is_err() {
        // Never render errors containing requests, paths or native diagnostics.
        eprintln!("runtime protocol failed");
        std::process::exit(1);
    }
}
