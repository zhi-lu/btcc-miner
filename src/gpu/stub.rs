// Stub GPU miner for non-macOS platforms.
// Always returns None from new().

use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, Mutex};

use crate::job::MiningJob;
use crate::stratum::{FoundShare, Subscription};

#[derive(Clone)]
pub struct GpuMiner;

impl GpuMiner {
    pub fn new() -> Option<Self> {
        eprintln!("GPU mining is only available on macOS with --features metal-gpu");
        None
    }

    #[allow(unused)]
    pub fn run(
        &self,
        _current_job: Arc<Mutex<Option<(MiningJob, [u8; 32])>>>,
        _running: Arc<AtomicBool>,
        _hashrate: Arc<Mutex<f64>>,
        _share_tx: mpsc::Sender<FoundShare>,
        _subscription: Arc<Mutex<Option<Subscription>>>,
    ) {
        // Stub: never called since new() returns None
    }
}
