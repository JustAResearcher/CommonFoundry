use std::collections::{BTreeMap, BTreeSet};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use cmfd_cuda::{CudaDevice, CudaLibrary};
use cmfd_node::p2p::{
    relay_blocks_to_peer_once_with_policy, spawn_inbound_listener_with_policy,
    spawn_static_peer_polling,
};
use cmfd_node::peer::{PeerAddressPolicy, PeerLimits, StaticPeerConfig};
use cmfd_node::{
    MiningJob, MiningShareSearchResult, Node, parse_miner_destination, unix_time_seconds,
};

const DEFAULT_MINER_DATA_DIR: &str = "commonfoundry-miner-devnet0";
const DEFAULT_MINER_P2P_ADDRESS: &str = "127.0.0.1:19444";
const DEFAULT_BATCH_SIZE: u32 = 8_192;
const MAX_BATCH_SIZE: u32 = 65_536;
const DEFAULT_STATS_SECONDS: u64 = 5;

#[derive(Debug, Parser)]
#[command(
    name = "cmfd-miner",
    version,
    about = "Common Foundry standalone multi-GPU CUDA miner"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List CUDA devices visible to the standalone miner.
    Devices {
        /// Path to the ForgeMatrix CUDA library. Defaults beside this executable.
        #[arg(long)]
        cuda_library: Option<PathBuf>,
    },
    /// Run continuous solo mining with an embedded Devnet node.
    Mine {
        #[arg(long, default_value = DEFAULT_MINER_DATA_DIR)]
        data_dir: PathBuf,
        #[arg(long, default_value = DEFAULT_MINER_P2P_ADDRESS)]
        p2p_bind: SocketAddr,
        /// Static Devnet peer. Repeat to configure more than one.
        #[arg(long = "peer")]
        peers: Vec<SocketAddr>,
        /// Allow numeric public peer addresses for Devnet testing.
        #[arg(long)]
        allow_public_peers: bool,
        /// CUDA device index. Repeat to select several; omitted means every supported GPU.
        #[arg(long = "device")]
        devices: Vec<i32>,
        /// Path to the ForgeMatrix CUDA library. Defaults beside this executable.
        #[arg(long)]
        cuda_library: Option<PathBuf>,
        /// Nonces evaluated per CUDA launch, from 1 through 65536.
        #[arg(long, default_value_t = DEFAULT_BATCH_SIZE)]
        batch_size: u32,
        /// 32-byte x-only Schnorr payout key as 64 hexadecimal characters.
        #[arg(long)]
        miner: Option<String>,
        /// Seconds between rig-rate reports.
        #[arg(long, default_value_t = DEFAULT_STATS_SECONDS)]
        stats_seconds: u64,
    },
}

#[derive(Debug)]
enum WorkerMessage {
    Ready {
        device: i32,
    },
    Progress {
        device: i32,
        attempts: u64,
    },
    Found {
        device: i32,
        block: Box<cmfd_consensus::Block>,
        attempts: u64,
    },
    Failed {
        device: i32,
        error: String,
    },
}

enum JobOutcome {
    Found {
        device: i32,
        block: Box<cmfd_consensus::Block>,
    },
    Stale,
    Shutdown,
}

struct WorkerSet {
    cancel: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
}

struct WorkerSpec {
    cuda: CudaLibrary,
    device: CudaDevice,
    ordinal: usize,
    worker_count: usize,
    job: MiningJob,
    batch_size: u32,
}

impl WorkerSet {
    fn stop(self) -> Result<()> {
        self.cancel.store(true, Ordering::Release);
        for handle in self.handles {
            handle
                .join()
                .map_err(|_| anyhow!("a CUDA worker thread panicked"))?;
        }
        Ok(())
    }
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Devices { cuda_library } => list_devices(cuda_library.as_deref()),
        Command::Mine {
            data_dir,
            p2p_bind,
            peers,
            allow_public_peers,
            devices,
            cuda_library,
            batch_size,
            miner,
            stats_seconds,
        } => run_miner(MinerOptions {
            data_dir,
            p2p_bind,
            peers,
            allow_public_peers,
            requested_devices: devices,
            cuda_library,
            batch_size,
            miner,
            stats_seconds,
        }),
    }
}

fn load_cuda(path: Option<&Path>) -> Result<CudaLibrary> {
    CudaLibrary::load(path)
        .map_err(anyhow::Error::msg)?
        .ok_or_else(|| anyhow!("ForgeMatrix CUDA library was not found beside cmfd-miner"))
}

fn list_devices(path: Option<&Path>) -> Result<()> {
    let library = load_cuda(path)?;
    let devices = library.devices().map_err(anyhow::Error::msg)?;
    println!("CUDA library: {}", library.path().display());
    if devices.is_empty() {
        println!("No NVIDIA CUDA devices found.");
        return Ok(());
    }
    for device in devices {
        let memory_gib = device.total_memory_bytes as f64 / 1024_f64.powi(3);
        println!(
            "GPU {}: {} | {:.2} GiB | {}",
            device.index,
            device.label(),
            memory_gib,
            if device.is_supported() {
                "supported"
            } else {
                "requires CUDA compute capability 7.0+"
            }
        );
    }
    Ok(())
}

struct MinerOptions {
    data_dir: PathBuf,
    p2p_bind: SocketAddr,
    peers: Vec<SocketAddr>,
    allow_public_peers: bool,
    requested_devices: Vec<i32>,
    cuda_library: Option<PathBuf>,
    batch_size: u32,
    miner: Option<String>,
    stats_seconds: u64,
}

struct ContinuousMiningConfig {
    payout: [u8; 32],
    peers: StaticPeerConfig,
    batch_size: u32,
    stats_interval: Duration,
}

fn run_miner(options: MinerOptions) -> Result<()> {
    if options.batch_size == 0 || options.batch_size > MAX_BATCH_SIZE {
        bail!("--batch-size must be between 1 and {MAX_BATCH_SIZE}");
    }
    if options.stats_seconds == 0 {
        bail!("--stats-seconds must be greater than zero");
    }

    let cuda = load_cuda(options.cuda_library.as_deref())?;
    let available = cuda.devices().map_err(anyhow::Error::msg)?;
    let devices = select_devices(&available, &options.requested_devices)?;
    let address_policy = if options.allow_public_peers {
        PeerAddressPolicy::AllowPublic
    } else {
        PeerAddressPolicy::PrivateOnly
    };

    let mut node = Node::open(&options.data_dir)
        .with_context(|| format!("open miner data directory {}", options.data_dir.display()))?;
    node.set_public_peer_mode(options.allow_public_peers);
    let payout = match options.miner.as_deref() {
        Some(value) => parse_miner_destination(value)?,
        None => node.wallet_destination(),
    };
    let shared = Arc::new(Mutex::new(node));
    let limits = PeerLimits::default();
    let p2p_socket = TcpListener::bind(options.p2p_bind)
        .with_context(|| format!("bind miner P2P listener at {}", options.p2p_bind))?;
    let p2p_address = p2p_socket.local_addr()?;
    let inbound = spawn_inbound_listener_with_policy(
        Arc::clone(&shared),
        p2p_socket,
        limits,
        address_policy,
    )?;
    let peer_config = StaticPeerConfig {
        listen_address: p2p_address,
        peers: options.peers,
        limits,
        address_policy,
    };
    let poller = if peer_config.peers.is_empty() {
        None
    } else {
        Some(spawn_static_peer_polling(
            Arc::clone(&shared),
            peer_config.clone(),
            Duration::from_secs(2),
        )?)
    };

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_handler = Arc::clone(&shutdown);
    ctrlc::set_handler(move || shutdown_handler.store(true, Ordering::Release))?;

    println!(
        "Common Foundry standalone CUDA miner v{}",
        env!("CARGO_PKG_VERSION")
    );
    println!("CUDA library: {}", cuda.path().display());
    println!("P2P listener: {p2p_address}");
    println!("Payout: {}", hex::encode(payout));
    println!("Using {} GPU(s):", devices.len());
    for device in &devices {
        println!("  GPU {}: {}", device.index, device.label());
    }
    println!("Press Ctrl+C to stop.\n");

    let mining_result = continuous_mining(
        Arc::clone(&shared),
        cuda,
        devices,
        ContinuousMiningConfig {
            payout,
            peers: peer_config,
            batch_size: options.batch_size,
            stats_interval: Duration::from_secs(options.stats_seconds),
        },
        shutdown,
    );
    let poll_result = match poller {
        Some(poller) => poller.stop().map_err(anyhow::Error::from),
        None => Ok(()),
    };
    let inbound_result = inbound.stop().map_err(anyhow::Error::from);
    mining_result?;
    poll_result?;
    inbound_result?;
    Ok(())
}

fn continuous_mining(
    node: Arc<Mutex<Node>>,
    cuda: CudaLibrary,
    devices: Vec<CudaDevice>,
    config: ContinuousMiningConfig,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    let mut blocks_found = 0_u64;
    while !shutdown.load(Ordering::Acquire) {
        let job = {
            let node = node.lock().map_err(|_| anyhow!("node mutex is poisoned"))?;
            node.build_mining_job(config.payout, unix_time_seconds()?)?
        };
        println!(
            "Mining height {} on {} GPU(s)...",
            job.challenge().height,
            devices.len()
        );
        match mine_job(
            Arc::clone(&node),
            cuda.clone(),
            devices.clone(),
            job,
            config.batch_size,
            config.stats_interval,
            Arc::clone(&shutdown),
        )? {
            JobOutcome::Shutdown => break,
            JobOutcome::Stale => println!("New chain tip received; rebuilding work."),
            JobOutcome::Found { device, block } => {
                let block_id = block.block_id();
                let height = block.challenge.height;
                let expected_parent = hex::encode(block.challenge.previous_block);
                let accepted = {
                    let mut node = node.lock().map_err(|_| anyhow!("node mutex is poisoned"))?;
                    if node.status()?.tip != expected_parent {
                        false
                    } else {
                        node.submit_block(*block, unix_time_seconds()?)?;
                        true
                    }
                };
                if accepted {
                    blocks_found = blocks_found.saturating_add(1);
                    let relayed = config
                        .peers
                        .peers
                        .iter()
                        .filter(|peer| {
                            relay_blocks_to_peer_once_with_policy(
                                Arc::clone(&node),
                                **peer,
                                config.peers.limits,
                                config.peers.address_policy,
                            )
                            .is_ok_and(|report| report.peer_tip == block_id)
                        })
                        .count();
                    if config.peers.peers.is_empty() || relayed == config.peers.peers.len() {
                        let relay = if config.peers.peers.is_empty() {
                            "local mode".to_owned()
                        } else {
                            format!("node sync {relayed}/{}", config.peers.peers.len())
                        };
                        println!(
                            "BLOCK FOUND | GPU {device} | height {height} | {} | {relay} | session blocks {blocks_found}",
                            hex::encode(block_id)
                        );
                    } else {
                        println!(
                            "BLOCK FOUND LOCALLY | GPU {device} | height {height} | {} | node sync pending {relayed}/{} (automatic retry) | session blocks {blocks_found}",
                            hex::encode(block_id),
                            config.peers.peers.len()
                        );
                    }
                } else {
                    println!("Candidate became stale before submission; rebuilding work.");
                }
            }
        }
    }
    println!("Miner stopped.");
    Ok(())
}

fn mine_job(
    node: Arc<Mutex<Node>>,
    cuda: CudaLibrary,
    devices: Vec<CudaDevice>,
    job: MiningJob,
    batch_size: u32,
    stats_interval: Duration,
    shutdown: Arc<AtomicBool>,
) -> Result<JobOutcome> {
    let worker_cancel = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::sync_channel(devices.len().saturating_mul(4).max(4));
    let handles = devices
        .iter()
        .enumerate()
        .map(|(ordinal, device)| {
            spawn_worker(
                WorkerSpec {
                    cuda: cuda.clone(),
                    device: device.clone(),
                    ordinal,
                    worker_count: devices.len(),
                    job: job.clone(),
                    batch_size,
                },
                Arc::clone(&worker_cancel),
                sender.clone(),
            )
        })
        .collect();
    drop(sender);
    let workers = WorkerSet {
        cancel: Arc::clone(&worker_cancel),
        handles,
    };

    let outcome = monitor_workers(
        node,
        &devices,
        &job,
        &receiver,
        stats_interval,
        shutdown,
        worker_cancel,
    );
    workers.stop()?;
    outcome
}

fn spawn_worker(
    spec: WorkerSpec,
    cancel: Arc<AtomicBool>,
    sender: SyncSender<WorkerMessage>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name(format!("cmfd-gpu-{}", spec.device.index))
        .spawn(move || {
            if let Err(error) = run_worker(&spec, &cancel, &sender) {
                cancel.store(true, Ordering::Release);
                let _ = sender.send(WorkerMessage::Failed {
                    device: spec.device.index,
                    error,
                });
            }
        })
        .expect("named CUDA worker thread creation should succeed")
}

fn run_worker(
    spec: &WorkerSpec,
    cancel: &AtomicBool,
    sender: &SyncSender<WorkerMessage>,
) -> Result<(), String> {
    let model = spec
        .job
        .accelerator_model()
        .map_err(|error| error.client_error().message)?;
    let mut miner = spec.cuda.create(&model, spec.device.index)?;
    let canary = spec
        .job
        .prepare_accelerator_batch(spec.ordinal as u64, 1)
        .map_err(|error| error.client_error().message)?;
    let canary_output = miner.evaluate(&canary)?;
    spec.job
        .verify_accelerator_output(&canary, 0, &canary_output)
        .map_err(|error| {
            format!(
                "CUDA differential check failed: {}",
                error.client_error().message
            )
        })?;
    sender
        .send(WorkerMessage::Ready {
            device: spec.device.index,
        })
        .map_err(|_| "miner coordinator closed during startup".to_owned())?;

    let stride = nonce_stride(spec.batch_size, spec.worker_count);
    let mut next_nonce = nonce_start(spec.batch_size, spec.ordinal);
    let mut pending_attempts = 0_u64;
    while !cancel.load(Ordering::Acquire) {
        let batch = spec
            .job
            .prepare_accelerator_batch(next_nonce, spec.batch_size)
            .map_err(|error| error.client_error().message)?;
        let outputs = miner.evaluate(&batch)?;
        pending_attempts = pending_attempts.saturating_add(u64::from(spec.batch_size));
        let result = spec
            .job
            .complete_accelerator_batch(&batch, &outputs, spec.job.challenge().target)
            .map_err(|error| error.client_error().message)?;
        match result {
            MiningShareSearchResult::Found { proof, .. } => {
                let block = spec
                    .job
                    .build_block_if_chain_valid(&proof)
                    .map_err(|error| error.client_error().message)?
                    .ok_or_else(|| "CUDA candidate did not meet the chain target".to_owned())?;
                cancel.store(true, Ordering::Release);
                sender
                    .send(WorkerMessage::Found {
                        device: spec.device.index,
                        block,
                        attempts: pending_attempts,
                    })
                    .map_err(|_| "miner coordinator closed before block submission".to_owned())?;
                return Ok(());
            }
            MiningShareSearchResult::Exhausted { .. } => {}
            MiningShareSearchResult::Cancelled { .. } => return Ok(()),
        }

        if sender
            .try_send(WorkerMessage::Progress {
                device: spec.device.index,
                attempts: pending_attempts,
            })
            .is_ok()
        {
            pending_attempts = 0;
        }
        next_nonce = next_nonce.wrapping_add(stride);
    }
    Ok(())
}

fn monitor_workers(
    node: Arc<Mutex<Node>>,
    devices: &[CudaDevice],
    job: &MiningJob,
    receiver: &Receiver<WorkerMessage>,
    stats_interval: Duration,
    shutdown: Arc<AtomicBool>,
    worker_cancel: Arc<AtomicBool>,
) -> Result<JobOutcome> {
    let mut ready = BTreeSet::new();
    let mut totals: BTreeMap<i32, u64> = devices.iter().map(|device| (device.index, 0)).collect();
    let mut last_totals = totals.clone();
    let mut last_report = Instant::now();
    let expected_parent = hex::encode(job.challenge().previous_block);

    loop {
        if shutdown.load(Ordering::Acquire) {
            worker_cancel.store(true, Ordering::Release);
            return Ok(JobOutcome::Shutdown);
        }
        match receiver.recv_timeout(Duration::from_millis(250)) {
            Ok(WorkerMessage::Ready { device }) => {
                ready.insert(device);
                println!(
                    "GPU {device} initialized ({}/{})",
                    ready.len(),
                    devices.len()
                );
            }
            Ok(WorkerMessage::Progress { device, attempts }) => {
                *totals.entry(device).or_default() = totals
                    .get(&device)
                    .copied()
                    .unwrap_or_default()
                    .saturating_add(attempts);
            }
            Ok(WorkerMessage::Found {
                device,
                block,
                attempts,
            }) => {
                *totals.entry(device).or_default() = totals
                    .get(&device)
                    .copied()
                    .unwrap_or_default()
                    .saturating_add(attempts);
                worker_cancel.store(true, Ordering::Release);
                return Ok(JobOutcome::Found { device, block });
            }
            Ok(WorkerMessage::Failed { device, error }) => {
                worker_cancel.store(true, Ordering::Release);
                bail!("GPU {device} failed: {error}");
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("all CUDA workers exited before finding or cancelling work")
            }
        }

        if last_report.elapsed() >= stats_interval {
            let elapsed = last_report.elapsed().as_secs_f64().max(f64::EPSILON);
            let mut total_rate = 0.0;
            let mut details = Vec::with_capacity(devices.len());
            for device in devices {
                let total = totals.get(&device.index).copied().unwrap_or_default();
                let previous = last_totals.get(&device.index).copied().unwrap_or_default();
                let rate = total.saturating_sub(previous) as f64 / elapsed;
                total_rate += rate;
                details.push(format!("GPU {}: {:.2} eval/s", device.index, rate));
                last_totals.insert(device.index, total);
            }
            println!(
                "Rig: {:.2} eval/s | {} | total attempts {}",
                total_rate,
                details.join(" | "),
                totals.values().copied().sum::<u64>()
            );
            last_report = Instant::now();
        }

        let current_tip = node
            .lock()
            .map_err(|_| anyhow!("node mutex is poisoned"))?
            .status()?
            .tip;
        if current_tip != expected_parent {
            worker_cancel.store(true, Ordering::Release);
            return Ok(JobOutcome::Stale);
        }
    }
}

fn select_devices(available: &[CudaDevice], requested: &[i32]) -> Result<Vec<CudaDevice>> {
    let by_index: BTreeMap<_, _> = available
        .iter()
        .cloned()
        .map(|device| (device.index, device))
        .collect();
    let indices: Vec<_> = if requested.is_empty() {
        available
            .iter()
            .filter(|device| device.is_supported())
            .map(|device| device.index)
            .collect()
    } else {
        let unique: BTreeSet<_> = requested.iter().copied().collect();
        if unique.len() != requested.len() {
            bail!("--device contains a duplicate CUDA index");
        }
        unique.into_iter().collect()
    };
    if indices.is_empty() {
        bail!("no supported CUDA devices were selected");
    }
    indices
        .into_iter()
        .map(|index| {
            let device = by_index
                .get(&index)
                .cloned()
                .ok_or_else(|| anyhow!("CUDA device {index} was not found"))?;
            if !device.is_supported() {
                bail!(
                    "CUDA device {index} has compute capability {}.{}, below the required 7.0",
                    device.compute_major,
                    device.compute_minor
                );
            }
            Ok(device)
        })
        .collect()
}

fn nonce_start(batch_size: u32, ordinal: usize) -> u64 {
    u64::from(batch_size).wrapping_mul(ordinal as u64)
}

fn nonce_stride(batch_size: u32, workers: usize) -> u64 {
    u64::from(batch_size).wrapping_mul(workers as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(index: i32, major: u32, minor: u32) -> CudaDevice {
        CudaDevice {
            index,
            name: format!("GPU {index}"),
            compute_major: major,
            compute_minor: minor,
            total_memory_bytes: 1,
        }
    }

    #[test]
    fn automatic_selection_uses_every_supported_gpu() {
        let available = vec![device(0, 6, 1), device(1, 7, 0), device(2, 12, 0)];
        let selected = select_devices(&available, &[]).unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|device| device.index)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn explicit_selection_is_sorted_and_rejects_duplicates() {
        let available = vec![device(0, 7, 5), device(1, 8, 6), device(2, 8, 9)];
        let selected = select_devices(&available, &[2, 0]).unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|device| device.index)
                .collect::<Vec<_>>(),
            vec![0, 2]
        );
        assert!(select_devices(&available, &[1, 1]).is_err());
    }

    #[test]
    fn gpu_nonce_ranges_do_not_overlap() {
        let batch_size = 4_u32;
        let workers = 3_usize;
        let stride = nonce_stride(batch_size, workers);
        let mut observed = BTreeSet::new();
        for round in 0..1_000_u64 {
            for ordinal in 0..workers {
                let start = nonce_start(batch_size, ordinal).wrapping_add(round * stride);
                for offset in 0..u64::from(batch_size) {
                    assert!(observed.insert(start.wrapping_add(offset)));
                }
            }
        }
        assert_eq!(observed.len(), 12_000);
    }
}
