// Stub GPU miner for non-macOS platforms.
// Always returns None from new().

use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, Mutex};

use crate::job::MiningJob;
use crate::stratum::{FoundShare, Subscription};

#[derive(Clone)]
pub struct GpuMiner;

impl GpuMiner {
    pub fn new() -> Vec<Self> {
        eprintln!("GPU mining is only available on macOS with --features metal-gpu");
        vec![]
    }

    #[allow(unused)]
    pub fn run(
        &self,
        _current_job: Arc<Mutex<Option<MiningJob>>>,
        _running: Arc<AtomicBool>,
        _hashrate: Arc<Mutex<f64>>,
        _share_tx: mpsc::Sender<FoundShare>,
        _subscription: Arc<Mutex<Option<Subscription>>>,
        _difficulty: Arc<Mutex<f64>>,
    ) {
        // Stub: never called since new() returns None
    }
}
