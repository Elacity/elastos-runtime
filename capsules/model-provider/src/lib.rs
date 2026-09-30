mod adapters;
mod config;
mod contract;
mod execution;
mod journal;
mod local_llama;
mod local_memory;
mod local_resources;
mod process;
mod state;
#[cfg(test)]
mod test_support;

pub use process::run_main;
