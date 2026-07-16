use std::{env, fs};

fn main() -> anyhow::Result<()> {
    let input = env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("missing input core wasm path"))?;
    let output = env::args()
        .nth(2)
        .ok_or_else(|| anyhow::anyhow!("missing output component path"))?;
    let core = fs::read(input)?;
    let component = wit_component::ComponentEncoder::default()
        .module(&core)?
        .validate(true)
        .encode()?;
    fs::write(output, component)?;
    Ok(())
}
