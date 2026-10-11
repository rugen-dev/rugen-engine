//! rugen binary.

#![forbid(unsafe_code)]

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut state = rugen_sim::State::new(1);
    state.step();
    log::info!(
        "rugen {} — sim frame {}",
        env!("CARGO_PKG_VERSION"),
        state.frame
    );
    Ok(())
}
