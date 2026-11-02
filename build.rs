use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/agent.proto");
    println!("cargo:rerun-if-changed=agent");
    println!("cargo:rerun-if-env-changed=ADBTOP_AGENT_APK");
    let descriptors = protox::compile(["proto/agent.proto"], ["proto"])?;
    prost_build::Config::new().compile_fds(descriptors)?;

    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let apk = std::env::var_os("ADBTOP_AGENT_APK")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("agent").join("adbtop-agent.apk"));
    let code = if apk.is_file() {
        println!("cargo:rerun-if-changed={}", apk.display());
        format!(
            "pub const BUNDLED_APK: Option<&[u8]> = Some(include_bytes!({:?}));\n",
            apk.canonicalize()?
        )
    } else {
        "pub const BUNDLED_APK: Option<&[u8]> = None;\n".to_string()
    };
    std::fs::write(
        PathBuf::from(std::env::var("OUT_DIR")?).join("agent_apk.rs"),
        code,
    )?;
    Ok(())
}
